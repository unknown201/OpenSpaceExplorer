//! Model-space rigs and glTF animation, independent of WebGPU and the browser.
//! Assets are shared; poses and playback belong to each draw/part instance.
use glam::{Mat4, Quat, Vec3, Vec4};
use serde_json::Value;

#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    pub fn matrix(self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
    pub fn blend(self, other: Self, weight: f32) -> Self {
        let t = weight.clamp(0.0, 1.0);
        Self {
            translation: self.translation.lerp(other.translation, t),
            rotation: self.rotation.slerp(other.rotation, t).normalize(),
            scale: self.scale.lerp(other.scale, t),
        }
    }
}

#[derive(Debug)]
pub struct Node {
    pub name: String,
    pub parent: Option<usize>,
    pub rest: Transform,
    /// Matrix nodes cannot be animated by glTF channels, but retain exact shear at rest.
    matrix: Option<Mat4>,
    pub mesh: Option<usize>,
    pub skin: Option<usize>,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Property {
    Translation,
    Rotation,
    Scale,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Interpolation {
    Step,
    Linear,
    Cubic,
}

#[derive(Debug)]
struct Channel {
    node: usize,
    property: Property,
    interpolation: Interpolation,
    times: Vec<f32>,
    values: Vec<Vec4>,
}

impl Channel {
    fn sample(&self, time: f32) -> Vec4 {
        let n = self.times.len();
        let cubic = self.interpolation == Interpolation::Cubic;
        let value = |i: usize| self.values[if cubic { i * 3 + 1 } else { i }];
        if time <= self.times[0] {
            return value(0);
        }
        if time >= self.times[n - 1] {
            return value(n - 1);
        }
        let b = self.times.partition_point(|t| *t <= time);
        let a = b - 1;
        let dt = self.times[b] - self.times[a];
        let t = (time - self.times[a]) / dt;
        match self.interpolation {
            Interpolation::Step => value(a),
            Interpolation::Linear if self.property == Property::Rotation => Vec4::from_array(
                Quat::from_array(value(a).to_array())
                    .normalize()
                    .slerp(Quat::from_array(value(b).to_array()).normalize(), t)
                    .to_array(),
            ),
            Interpolation::Linear => value(a).lerp(value(b), t),
            Interpolation::Cubic => {
                let t2 = t * t;
                let t3 = t2 * t;
                value(a) * (2.0 * t3 - 3.0 * t2 + 1.0)
                    + self.values[a * 3 + 2] * dt * (t3 - 2.0 * t2 + t)
                    + value(b) * (-2.0 * t3 + 3.0 * t2)
                    + self.values[b * 3] * dt * (t3 - t2)
            }
        }
    }
}

#[derive(Debug)]
pub struct Clip {
    pub name: String,
    pub duration: f32,
    channels: Vec<Channel>,
}

#[derive(Debug)]
pub struct Skin {
    pub joints: Vec<usize>,
    pub inverse_bind: Vec<Mat4>,
}

#[derive(Clone, Copy, Debug)]
pub struct ClipSample {
    pub clip: usize,
    pub time: f32,
    pub looping: bool,
}

#[derive(Debug)]
pub struct Rig {
    pub nodes: Vec<Node>,
    pub clips: Vec<Clip>,
    pub skins: Vec<Skin>,
    order: Vec<usize>,
    /// Exported explicitly by the converter, never guessed by the renderer from part names.
    pub deployment: Option<(usize, bool)>,
    pub deployment_control: String,
}

impl Rig {
    pub fn from_gltf(json: &Value, bin: &[u8]) -> Result<Self, String> {
        let read = Accessors { json, bin };
        let raw = json["nodes"].as_array().cloned().unwrap_or_default();
        let mut nodes = Vec::new();
        for n in &raw {
            let translation = array::<3>(&n["translation"], [0.0; 3])?;
            let rotation = array::<4>(&n["rotation"], [0.0, 0.0, 0.0, 1.0])?;
            let scale = array::<3>(&n["scale"], [1.0; 3])?;
            let q = Quat::from_array(rotation);
            if q.length_squared() < 1e-12 {
                return Err("zero node quaternion".into());
            }
            let matrix = if n.get("matrix").is_some() {
                Some(Mat4::from_cols_array(&array::<16>(
                    &n["matrix"],
                    [0.0; 16],
                )?))
            } else {
                None
            };
            nodes.push(Node {
                name: n["name"].as_str().unwrap_or("").into(),
                parent: None,
                rest: Transform {
                    translation: translation.into(),
                    rotation: q.normalize(),
                    scale: scale.into(),
                },
                matrix,
                mesh: index(&n["mesh"]),
                skin: index(&n["skin"]),
                visible: false,
            });
        }
        for (i, n) in raw.iter().enumerate() {
            if let Some(children) = n["children"].as_array() {
                for child in children {
                    let j = index(child).ok_or("node child index")?;
                    let child = nodes.get_mut(j).ok_or("node child out of range")?;
                    if child.parent.replace(i).is_some() {
                        return Err("node has multiple parents".into());
                    }
                }
            }
        }
        // Kahn traversal both validates cycles and avoids recursive stack growth on imported assets.
        let mut order: Vec<usize> = nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| n.parent.is_none().then_some(i))
            .collect();
        let mut at = 0;
        while at < order.len() {
            if let Some(children) = raw[order[at]]["children"].as_array() {
                order.extend(children.iter().filter_map(index));
            }
            at += 1;
        }
        if order.len() != nodes.len() {
            return Err("node cycle".into());
        }
        let scene = index(&json["scene"]).unwrap_or(0);
        if let Some(roots) = json["scenes"][scene]["nodes"].as_array() {
            for r in roots {
                let i = index(r).ok_or("scene root index")?;
                let n = nodes.get_mut(i).ok_or("scene root out of range")?;
                if n.parent.is_some() {
                    return Err("scene root has a parent".into());
                }
                n.visible = true;
            }
        }
        for &i in &order {
            if let Some(p) = nodes[i].parent {
                nodes[i].visible |= nodes[p].visible;
            }
        }
        let mut skins = Vec::new();
        for s in json["skins"].as_array().into_iter().flatten() {
            let joints: Vec<usize> = s["joints"]
                .as_array()
                .ok_or("skin joints")?
                .iter()
                .map(|v| {
                    index(v)
                        .filter(|i| *i < nodes.len())
                        .ok_or("skin joint out of range".to_string())
                })
                .collect::<Result<_, _>>()?;
            let inverse_bind = if let Some(a) = index(&s["inverseBindMatrices"]) {
                let data = read.floats(a, 16)?;
                if data.len() != joints.len() * 16 {
                    return Err("inverse bind count".into());
                }
                data.as_chunks::<16>()
                    .0
                    .iter()
                    .map(|v| Mat4::from_cols_slice(v))
                    .collect()
            } else {
                vec![Mat4::IDENTITY; joints.len()]
            };
            skins.push(Skin {
                joints,
                inverse_bind,
            });
        }
        if nodes
            .iter()
            .any(|n| n.skin.is_some_and(|i| i >= skins.len()))
        {
            return Err("node skin out of range".into());
        }
        let mut clips = Vec::new();
        for a in json["animations"].as_array().into_iter().flatten() {
            let mut channels = Vec::new();
            let mut duration = 0.0f32;
            for c in a["channels"].as_array().into_iter().flatten() {
                let node = index(&c["target"]["node"])
                    .filter(|i| *i < nodes.len())
                    .ok_or("animation target")?;
                if nodes[node].matrix.is_some() {
                    return Err("animation targets a matrix node".into());
                }
                let (property, width) = match c["target"]["path"].as_str() {
                    Some("translation") => (Property::Translation, 3),
                    Some("rotation") => (Property::Rotation, 4),
                    Some("scale") => (Property::Scale, 3),
                    _ => return Err("unsupported animation path (expected TRS)".into()),
                };
                let si = index(&c["sampler"]).ok_or("animation sampler index")?;
                let s = a["samplers"].get(si).ok_or("animation sampler")?;
                let interpolation = match s["interpolation"].as_str().unwrap_or("LINEAR") {
                    "LINEAR" => Interpolation::Linear,
                    "STEP" => Interpolation::Step,
                    "CUBICSPLINE" => Interpolation::Cubic,
                    _ => return Err("animation interpolation".into()),
                };
                let times = read.floats(index(&s["input"]).ok_or("animation input")?, 1)?;
                if times.is_empty() || times[0] < 0.0 || times.windows(2).any(|w| w[1] <= w[0]) {
                    return Err("invalid animation times".into());
                }
                let data = read.floats(index(&s["output"]).ok_or("animation output")?, width)?;
                let count = times.len()
                    * if interpolation == Interpolation::Cubic {
                        3
                    } else {
                        1
                    };
                if data.len() != count * width {
                    return Err("animation output count".into());
                }
                let values: Vec<Vec4> = data
                    .chunks_exact(width)
                    .map(|v| Vec4::new(v[0], v[1], v[2], if width == 4 { v[3] } else { 0.0 }))
                    .collect();
                if property == Property::Rotation
                    && values.iter().enumerate().any(|(i, v)| {
                        (interpolation != Interpolation::Cubic || i % 3 == 1)
                            && v.length_squared() < 1e-12
                    })
                {
                    return Err("zero animation quaternion".into());
                }
                duration = duration.max(*times.last().unwrap());
                channels.push(Channel {
                    node,
                    property,
                    interpolation,
                    times,
                    values,
                });
            }
            clips.push(Clip {
                name: a["name"].as_str().unwrap_or("").into(),
                duration,
                channels,
            });
        }
        let deploy = &json["extras"]["ose_deployment"];
        let deployment = index(&deploy["clip"])
            .filter(|i| *i < clips.len())
            .map(|i| (i, deploy["reverse"].as_bool().unwrap_or(false)));
        Ok(Self {
            nodes,
            clips,
            skins,
            order,
            deployment,
            deployment_control: json["extras"]["ose_deployment"]["control"]
                .as_str()
                .unwrap_or("")
                .into(),
        })
    }

    pub fn pose(&self, sample: Option<ClipSample>) -> Vec<Transform> {
        let mut pose: Vec<_> = self.nodes.iter().map(|n| n.rest).collect();
        if let Some((sample, clip)) = sample.and_then(|s| self.clips.get(s.clip).map(|c| (s, c))) {
            let time = if !sample.time.is_finite() {
                0.0
            } else if sample.looping && clip.duration > 0.0 {
                sample.time.rem_euclid(clip.duration)
            } else {
                sample.time.clamp(0.0, clip.duration)
            };
            for c in &clip.channels {
                let v = c.sample(time);
                match c.property {
                    Property::Translation => pose[c.node].translation = v.truncate(),
                    Property::Scale => pose[c.node].scale = v.truncate(),
                    Property::Rotation => {
                        pose[c.node].rotation = if v.length_squared() > 1e-12 {
                            Quat::from_array(v.normalize().to_array())
                        } else {
                            self.nodes[c.node].rest.rotation
                        }
                    }
                }
            }
        }
        pose
    }

    pub fn world_matrices(&self, pose: &[Transform]) -> Vec<Mat4> {
        let mut world = vec![Mat4::IDENTITY; self.nodes.len()];
        for &i in &self.order {
            let local = self.nodes[i]
                .matrix
                .unwrap_or_else(|| pose.get(i).copied().unwrap_or(self.nodes[i].rest).matrix());
            world[i] = self.nodes[i].parent.map_or(local, |p| world[p] * local);
        }
        world
    }

    /// Override layer with an optional per-node mask (e.g. face/upper body over locomotion).
    pub fn blend_pose(
        &self,
        base: &mut [Transform],
        layer: &[Transform],
        weight: f32,
        mask: Option<&[f32]>,
    ) {
        for (i, (base, layer)) in base.iter_mut().zip(layer).enumerate() {
            let weight = weight * mask.map_or(1.0, |m| m.get(i).copied().unwrap_or(0.0));
            if weight.is_finite() {
                *base = base.blend(*layer, weight);
            }
        }
    }

    /// Late aim constraint in model space. Run after clip blending and before palette construction.
    /// `forward` is the constrained node's local facing axis; zero-length targets leave the pose intact.
    pub fn aim_at(
        &self,
        pose: &mut [Transform],
        node: usize,
        target: Vec3,
        forward: Vec3,
        weight: f32,
    ) {
        if node >= pose.len()
            || node >= self.nodes.len()
            || !target.is_finite()
            || !weight.is_finite()
        {
            return;
        }
        let world = self.world_matrices(pose);
        let direction = target - world[node].transform_point3(Vec3::ZERO);
        let facing = world[node].transform_vector3(forward).normalize_or_zero();
        if direction.length_squared() < 1e-12 || facing.length_squared() < 1e-12 {
            return;
        }
        let (_, rotation, _) = world[node].to_scale_rotation_translation();
        let parent = self.nodes[node].parent.map_or(Quat::IDENTITY, |p| {
            world[p].to_scale_rotation_translation().1
        });
        let target =
            parent.inverse() * Quat::from_rotation_arc(facing, direction.normalize()) * rotation;
        pose[node].rotation = pose[node]
            .rotation
            .slerp(target, weight.clamp(0.0, 1.0))
            .normalize();
    }

    /// Model-space position constraint, including parents with rotation or scale.
    pub fn copy_position(&self, pose: &mut [Transform], node: usize, source: usize, weight: f32) {
        if node >= pose.len()
            || node >= self.nodes.len()
            || source >= self.nodes.len()
            || !weight.is_finite()
        {
            return;
        }
        let world = self.world_matrices(pose);
        let mut target = world[source].transform_point3(Vec3::ZERO);
        if let Some(p) = self.nodes[node].parent {
            if world[p].determinant().abs() < 1e-12 {
                return;
            }
            target = world[p].inverse().transform_point3(target);
        }
        pose[node].translation = pose[node].translation.lerp(target, weight.clamp(0.0, 1.0));
    }

    /// glTF joint palette in the skinned mesh's local frame. The draw's model transform applies once.
    pub fn palette(&self, node: usize, world: &[Mat4]) -> Option<Vec<Mat4>> {
        let skin = self.skins.get(self.nodes.get(node)?.skin?)?;
        let mesh = *world.get(node)?;
        if mesh.determinant().abs() < 1e-12 {
            return None;
        }
        let inverse = mesh.inverse();
        Some(
            skin.joints
                .iter()
                .zip(&skin.inverse_bind)
                .map(|(&j, b)| inverse * world[j] * *b)
                .collect(),
        )
    }

    pub fn deployment_sample(&self, fraction: f32) -> Option<ClipSample> {
        let (clip, reverse) = self.deployment?;
        let fraction = fraction.clamp(0.0, 1.0);
        Some(ClipSample {
            clip,
            time: self.clips[clip].duration * if reverse { 1.0 - fraction } else { fraction },
            looping: false,
        })
    }

    /// Only supported part controllers have a commanded initial pose. Other rigs retain their
    /// authored rest pose until a caller explicitly samples them (e.g. engine mode/reverser clips).
    pub fn default_sample(&self) -> Option<ClipSample> {
        match self.deployment_control.as_str() {
            "panels" | "gear" => self.deployment_sample(0.0),
            _ => None,
        }
    }
}

/// A reversible deployment timeline. First observation snaps to authoritative state (late join/reload).
#[derive(Clone, Copy, Debug)]
pub struct Deployment {
    fraction: f32,
    last_time: f64,
    target: bool,
}
impl Deployment {
    pub fn new(deployed: bool, time: f64) -> Self {
        Self {
            fraction: if deployed { 1.0 } else { 0.0 },
            target: deployed,
            last_time: time,
        }
    }
    pub fn update(&mut self, deployed: bool, time: f64, duration: f32) -> f32 {
        // Advance the old target before accepting a new command, so reversal never jumps.
        let dt = (time - self.last_time).max(0.0) as f32;
        if time.is_finite() {
            self.last_time = time;
        }
        let step = if duration > 0.0 { dt / duration } else { 1.0 };
        self.fraction = if self.target {
            (self.fraction + step).min(1.0)
        } else {
            (self.fraction - step).max(0.0)
        };
        self.target = deployed;
        self.fraction
    }
}

/// Linear blend skinning with normalized weights and inverse-transpose normals.
pub fn skin_vertex(
    pos: Vec3,
    normal: Vec3,
    joints: [u16; 4],
    weights: [f32; 4],
    palette: &[Mat4],
) -> (Vec3, Vec3) {
    let mut m = Mat4::ZERO;
    let mut total = 0.0;
    for (&j, &w) in joints.iter().zip(&weights) {
        if w.is_finite() && w > 0.0 {
            if let Some(p) = palette.get(j as usize) {
                m += *p * w;
                total += w;
            }
        }
    }
    if total <= 1e-12 {
        return (pos, normal);
    }
    m *= total.recip();
    let n = if m.determinant().abs() > 1e-12 {
        m.inverse()
            .transpose()
            .transform_vector3(normal)
            .normalize_or_zero()
    } else {
        normal
    };
    (m.transform_point3(pos), n)
}

fn index(v: &Value) -> Option<usize> {
    v.as_u64().and_then(|i| usize::try_from(i).ok())
}
fn array<const N: usize>(v: &Value, default: [f32; N]) -> Result<[f32; N], String> {
    if v.is_null() {
        return Ok(default);
    }
    let a = v
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or("invalid transform array")?;
    let mut out = default;
    for (i, v) in a.iter().enumerate() {
        out[i] = v.as_f64().ok_or("transform number")? as f32;
        if !out[i].is_finite() {
            return Err("nonfinite transform".into());
        }
    }
    Ok(out)
}

/// Checked glTF accessors for clips, joint indices, weights and inverse bind matrices.
pub struct Accessors<'a> {
    pub json: &'a Value,
    pub bin: &'a [u8],
}
impl Accessors<'_> {
    pub fn floats(&self, i: usize, width: usize) -> Result<Vec<f32>, String> {
        let a = self.json["accessors"].get(i).ok_or("accessor index")?;
        if a.get("sparse").is_some() {
            return Err("sparse accessor unsupported".into());
        }
        let ty = match width {
            1 => "SCALAR",
            2 => "VEC2",
            3 => "VEC3",
            4 => "VEC4",
            16 => "MAT4",
            _ => return Err("accessor width".into()),
        };
        if a["type"].as_str() != Some(ty) {
            return Err("accessor shape".into());
        }
        let view = index(&a["bufferView"])
            .and_then(|i| self.json["bufferViews"].get(i))
            .ok_or("accessor view")?;
        if index(&view["buffer"]).unwrap_or(0) != 0 {
            return Err("external buffer unsupported".into());
        }
        let kind = a["componentType"].as_u64().ok_or("component type")?;
        let size = match kind {
            5121 => 1,
            5123 => 2,
            5126 => 4,
            _ => return Err("accessor component unsupported".into()),
        };
        let start = index(&view["byteOffset"]).unwrap_or(0);
        let len = index(&view["byteLength"]).ok_or("view length")?;
        let data = self
            .bin
            .get(start..start.checked_add(len).ok_or("view overflow")?)
            .ok_or("view bounds")?;
        let offset = index(&a["byteOffset"]).unwrap_or(0);
        let count = index(&a["count"]).ok_or("accessor count")?;
        let stride = index(&view["byteStride"]).unwrap_or(width * size);
        if stride < width * size {
            return Err("accessor stride".into());
        }
        let end = if count == 0 {
            offset
        } else {
            (count - 1)
                .checked_mul(stride)
                .and_then(|v| v.checked_add(offset))
                .and_then(|v| v.checked_add(width * size))
                .ok_or("accessor overflow")?
        };
        if end > data.len() {
            return Err("accessor bounds".into());
        }
        let normalized = a["normalized"].as_bool().unwrap_or(false);
        let mut out = Vec::with_capacity(count * width);
        for row in 0..count {
            for col in 0..width {
                let at = offset + row * stride + col * size;
                let f = match kind {
                    5126 => f32::from_le_bytes(data[at..at + 4].try_into().unwrap()),
                    5123 => {
                        u16::from_le_bytes(data[at..at + 2].try_into().unwrap()) as f32
                            / if normalized { 65535.0 } else { 1.0 }
                    }
                    _ => data[at] as f32 / if normalized { 255.0 } else { 1.0 },
                };
                if !f.is_finite() {
                    return Err("nonfinite accessor".into());
                }
                out.push(f);
            }
        }
        Ok(out)
    }
}
