"""Port the arithmetic in KSP's compiled D3D exhaust shaders to WGSL.

The generated shader and source disassembly are game-derived assets, kept under assets/web/fx.
Unsupported instructions fail the export rather than silently replacing a shader with an approximation.
Windows' D3D compiler disassembles bytecode; this module supplies the explicit register semantics.
"""
from __future__ import annotations
import ctypes
import re
import struct


def disassemble(bytecode: bytes) -> str:
    dll = ctypes.WinDLL("d3dcompiler_47.dll")
    f = dll.D3DDisassemble
    f.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint, ctypes.c_char_p, ctypes.POINTER(ctypes.c_void_p)]
    f.restype = ctypes.c_long
    blob = ctypes.c_void_p()
    hr = f(bytecode, len(bytecode), 0, None, ctypes.byref(blob))
    if hr < 0:
        raise RuntimeError(f"D3DDisassemble failed: {hr:x}")
    vtable = ctypes.cast(blob, ctypes.POINTER(ctypes.POINTER(ctypes.c_void_p))).contents
    try:
        ptr = ctypes.WINFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p)(vtable[3])(blob)
        size = ctypes.WINFUNCTYPE(ctypes.c_size_t, ctypes.c_void_p)(vtable[4])(blob)
        return ctypes.string_at(ptr, size).decode().rstrip("\0")
    finally:
        ctypes.WINFUNCTYPE(ctypes.c_ulong, ctypes.c_void_p)(vtable[2])(blob)


def programs(shader) -> list[bytes]:
    from UnityPy.helpers.CompressionHelper import decompress_lz4
    offset, size, decoded = shader.offsets[0][0], shader.compressedLengths[0][0], shader.decompressedLengths[0][0]
    data = decompress_lz4(bytes(shader.compressedBlob)[offset:offset+size], decoded)
    result = []
    start = 0
    while (start := data.find(b"DXBC", start)) >= 0:
        size = struct.unpack_from("<I", data, start+24)[0]
        result.append(data[start:start+size])
        start += size
    return result


def operands(line: str) -> list[str]:
    return [x.strip() for x in re.split(r",\s*(?![^()]*\))", line)]


def source(value: str, stage: str) -> str:
    negative = value.startswith("-")
    if negative: value = value[1:]
    absolute = value.startswith("|")
    if absolute: value = value.strip("|")
    if value.startswith("l("):
        nums = operands(value[2:-1])
        if len(nums) == 1: nums *= 4
        def number(n):
            if n.startswith("0x"): return f"bitcast<f32>({n}u)"
            if "." not in n and "e" not in n.lower(): n += ".0"
            return n
        result = "vec4<f32>(" + ", ".join(map(number, nums)) + ")"
    else:
        match = re.fullmatch(r"(cb\d+\[\d+\]|[rvo]\d+)(?:\.([xyzw]+))?", value)
        if not match: raise ValueError(f"Unsupported source {value}")
        register, swizzle = match.groups()
        if register.startswith("cb"):
            cb, index = map(int, re.findall(r"\d+", register))
            register = f"u.{['globals','camera','object','frame'][cb]}[{index}]"
        swizzle = swizzle or "xyzw"
        if len(swizzle) < 4: swizzle += swizzle[-1] * (4-len(swizzle))
        result = f"{register}.{swizzle}"
    if absolute: result = f"abs({result})"
    if negative: result = f"(-{result})"
    return result


def translate(assembly: str, stage: str, textures: dict[int, str]) -> str:
    lines = []
    count = 0
    temps = int(re.search(r"dcl_temps (\d+)", assembly).group(1))
    for i in range(temps): lines.append(f"var r{i} = vec4<f32>(0.0);")
    for i in range(8 if stage == "vertex" else 1): lines.append(f"var o{i} = vec4<f32>(0.0);")
    def emit(dest, expression):
        nonlocal count
        if dest == "null": return
        register, _, mask = dest.partition(".")
        mask = mask or "xyzw"
        name = f"result{count}"
        lines.append(f"let {name} = {expression};")
        for component in mask: lines.append(f"{register}.{component} = {name}.{component};")
        count += 1
    for raw in assembly.splitlines():
        raw = raw.strip()
        if not raw or raw.startswith(("//", "dcl_", "vs_", "ps_")) or raw == "ret": continue
        op, args = raw.split(None, 1)
        args = operands(args)
        saturate = op.endswith("_sat")
        if saturate: op = op[:-4]
        dest = args[0]
        a = [source(arg, stage) for arg in args[1:]] if op not in ("sample", "sample_l", "sincos") else []
        if op in ("add", "mul", "div"): expression = f"({a[0]} {dict(add='+',mul='*',div='/')[op]} {a[1]})"
        elif op == "mad": expression = f"fma({a[0]}, {a[1]}, {a[2]})"
        elif op == "mov": expression = a[0]
        elif op in ("min", "max"): expression = f"{op}({a[0]}, {a[1]})"
        elif op.startswith("dp"): expression = f"vec4<f32>(dot(({a[0]}).{'xyzw'[:int(op[-1])]}, ({a[1]}).{'xyzw'[:int(op[-1])]}))"
        elif op in ("lt", "ge", "eq", "ne"):
            expression = f"bitcast<vec4<f32>>(select(vec4<u32>(0u), vec4<u32>(0xffffffffu), {a[0]} {dict(lt='<',ge='>=',eq='==',ne='!=')[op]} {a[1]}))"
        elif op == "iadd":
            # D3D integer negation is applied to the bit pattern, not to its float interpretation.
            ia = [f"{'-' if arg.startswith('-') else ''}bitcast<vec4<i32>>({source(arg.lstrip('-'),stage)})" for arg in args[1:]]
            expression = f"bitcast<vec4<f32>>({ia[0]} + {ia[1]})"
        elif op == "itof": expression = f"vec4<f32>(bitcast<vec4<i32>>({a[0]}))"
        elif op == "and": expression = f"bitcast<vec4<f32>>(bitcast<vec4<u32>>({a[0]}) & bitcast<vec4<u32>>({a[1]}))"
        elif op == "movc": expression = f"select({a[2]}, {a[1]}, bitcast<vec4<u32>>({a[0]}) != vec4<u32>(0u))"
        elif op in ("log", "exp", "sqrt", "rsq", "frc", "round_ni"):
            expression = f"{dict(log='log2',exp='exp2',sqrt='sqrt',rsq='inverseSqrt',frc='fract',round_ni='floor')[op]}({a[0]})"
        elif op == "sincos":
            arg = source(args[2], stage)
            angle = f"angle{count}"
            lines.append(f"let {angle} = {arg};")
            emit(dest, f"sin({angle})"); emit(args[1], f"cos({angle})")
            continue
        elif op in ("sample", "sample_l"):
            texture = int(re.match(r"t(\d+)",args[2]).group(1))
            tex = textures[texture]
            coords = source(args[1],stage)
            # Preserve implicit fragment LOD and the source's explicit vertex LOD.
            lod = f"({source(args[4],stage)}).x" if op == "sample_l" else "0.0"
            if tex == "depth_texture": expression = f"sample_depth(({coords}).xy)"
            else:
                sampler='screen_sampler' if tex=='scene_texture' else 'fx_sampler'
                expression=f"textureSampleLevel({tex}, {sampler}, ({coords}).xy, {lod})" if op=='sample_l' else f"textureSample({tex}, {sampler}, ({coords}).xy)"
        else: raise ValueError(f"Unsupported {stage} instruction: {raw}")
        if saturate: expression = f"clamp({expression}, vec4<f32>(0.0), vec4<f32>(1.0))"
        emit(dest, expression)
    if stage == "vertex": lines.append("return OriginalVertex(o0,o1,o2,o3,o4,o5,o6,o7);")
    else: lines.append("return o0;")
    return "\n".join("    "+line for line in lines)


def signature(assembly: str, which: str):
    part = assembly.split(which + " signature:")[1].split("//\n//\n")[0]
    return [(name, int(index), int(register)) for name,index,mask,register in
        re.findall(r"^//\s+(\w+)\s+(\d+)\s+([xyzw]+)\s+(\d+)\s+",part,re.M)]


def wgsl(vertex: str, fragment: str, vertex_textures: dict[int,str], fragment_textures: dict[int,str]) -> str:
    arguments = ",".join(f"v{i}: vec4<f32>" for i in range(8))
    code = """
struct FxUniform {
    globals: array<vec4<f32>,64>, camera: array<vec4<f32>,9>,
    object: array<vec4<f32>,11>, frame: array<vec4<f32>,23>, weights: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: FxUniform;
@group(1) @binding(0) var distortion_texture: texture_2d<f32>;
@group(1) @binding(1) var noise_texture: texture_2d<f32>;
@group(1) @binding(2) var traces_texture: texture_2d<f32>;
@group(1) @binding(3) var fx_sampler: sampler;
@group(2) @binding(0) var scene_texture: texture_2d<f32>;
@group(2) @binding(1) var depth_texture: texture_depth_2d;
@group(2) @binding(2) var screen_sampler: sampler;
fn sample_depth(uv: vec2<f32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(depth_texture));
    return vec4<f32>(textureLoad(depth_texture,clamp(vec2<i32>(uv*vec2<f32>(size)),vec2<i32>(0),size-1),0));
}
struct MeshInput {
    @location(0) position: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>,
    @location(3) delta0: vec3<f32>, @location(4) normal0: vec3<f32>,
    @location(5) delta1: vec3<f32>, @location(6) normal1: vec3<f32>,
    @location(7) delta2: vec3<f32>, @location(8) normal2: vec3<f32>,
};
struct RasterInput {
    @builtin(position) clip: vec4<f32>,
    @location(0) t0: vec4<f32>, @location(1) t1: vec4<f32>, @location(2) t2: vec4<f32>,
    @location(3) t3: vec4<f32>, @location(4) t4: vec4<f32>, @location(5) t5: vec4<f32>, @location(6) t6: vec4<f32>,
};
"""
    code += "struct OriginalVertex {" + ",".join(f"o{i}:vec4<f32>" for i in range(8)) + ",};\n"
    code += f"fn original_vertex({arguments}) -> OriginalVertex {{\n{translate(vertex,'vertex',vertex_textures)}\n}}\n"
    code += f"fn original_fragment({arguments}) -> vec4<f32> {{\n{translate(fragment,'fragment',fragment_textures)}\n}}\n"
    va = ["vec4<f32>(0.0)"]*8
    for name,index,register in signature(vertex,"Input"):
        if name == "POSITION": va[register] = "position"
        elif name == "NORMAL": va[register] = "normal"
        elif name == "TEXCOORD" and index == 0: va[register] = "vec4<f32>(in.uv,0.0,0.0)"
        elif name == "COLOR": va[register] = "vec4<f32>(1.0)"
    code += """
@vertex fn vs_plume(in: MeshInput) -> RasterInput {
    let position = vec4<f32>(in.position + in.delta0*u.weights.x + in.delta1*u.weights.y + in.delta2*u.weights.z,1.0);
    let normal = vec4<f32>(normalize(in.normal + in.normal0*u.weights.x + in.normal1*u.weights.y + in.normal2*u.weights.z),0.0);
"""
    code += f"    let original = original_vertex({','.join(va)});\n    var out: RasterInput;\n"
    for name,index,register in signature(vertex,"Output"):
        if name == "SV_POSITION": code += f"    out.clip = original.o{register};\n"
        elif name == "TEXCOORD": code += f"    out.t{index} = original.o{register};\n"
    code += "    return out;\n}\n"
    fa = ["vec4<f32>(0.0)"]*8
    for name,index,register in signature(fragment,"Input"):
        if name == "TEXCOORD": fa[register] = f"in.t{index}"
        elif name == "SV_IsFrontFace": fa[register] = "bitcast<vec4<f32>>(vec4<u32>(select(0u,0xffffffffu,front)))"
        elif name == "SV_POSITION": fa[register] = "in.clip"
    code += f"@fragment fn fs_plume(in: RasterInput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {{ return original_fragment({','.join(fa)}); }}\n"
    return code
