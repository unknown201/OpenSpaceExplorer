// Offline reference renderer: executes the game's original DXBC, without the WGSL port.
// Used by check_fx_parity.py. Build outputs and all game-derived inputs stay in .cache/.
#include <windows.h>
#include <d3d11.h>
#include <wrl/client.h>
#include <fstream>
#include <vector>
#include <array>
#include <cstdint>
#include <cmath>
#include <iostream>
#include <string>
#include <stdexcept>
using Microsoft::WRL::ComPtr;
void checked(HRESULT h,int line){if(FAILED(h))throw std::runtime_error("D3D failure "+std::to_string((unsigned)h)+" at line "+std::to_string(line));}
#define check(x) checked(x,__LINE__)
template<class T>T read(std::istream& f){T v;f.read((char*)&v,sizeof(v));if(!f)throw std::runtime_error("truncated reference input");return v;}
std::vector<char> bytes(std::istream& f,size_t n){std::vector<char>b(n);f.read(b.data(),n);if(!f)throw std::runtime_error("truncated reference input");return b;}
struct Shader{ComPtr<ID3D11VertexShader>vs;ComPtr<ID3D11PixelShader>ps;ComPtr<ID3D11InputLayout>layout;bool refraction,reentry;};
// The 512px reference grid contains exactly representable positive normal half values.
// This is not a general float-to-half converter.
uint16_t grid_half(float value){uint32_t x;memcpy(&x,&value,4);return(uint16_t)((((x>>23)-127+15)<<10)|((x&0x7fffff)>>13));}
struct Mesh{uint32_t vertices,indices;std::vector<float>v;ComPtr<ID3D11Buffer>ib;};
int main(int argc,char**argv){try{
 if(argc!=4)throw std::runtime_error("fx_reference ASSETS FRAME OUTPUT");
 ComPtr<ID3D11Device>dev;ComPtr<ID3D11DeviceContext>ctx;
 check(D3D11CreateDevice(nullptr,D3D_DRIVER_TYPE_HARDWARE,nullptr,0,nullptr,0,D3D11_SDK_VERSION,&dev,nullptr,&ctx));
 auto buffer=[&](const void*data,UINT size,UINT bind){ComPtr<ID3D11Buffer>b;D3D11_BUFFER_DESC d={};d.ByteWidth=size;d.Usage=D3D11_USAGE_DEFAULT;d.BindFlags=bind;
  D3D11_SUBRESOURCE_DATA s={};s.pSysMem=data;check(dev->CreateBuffer(&d,&s,&b));return b;};
 std::ifstream assets(argv[1],std::ios::binary);if(read<uint32_t>(assets)!=0x41525846)throw std::runtime_error("bad asset magic");
 auto nt=read<uint32_t>(assets),ns=read<uint32_t>(assets),nm=read<uint32_t>(assets);
 std::vector<ComPtr<ID3D11ShaderResourceView>>textures;
 for(unsigned i=0;i<nt;i++){
  UINT w=read<uint32_t>(assets),h=read<uint32_t>(assets),mips=read<uint32_t>(assets),format=read<uint32_t>(assets);
  D3D11_TEXTURE2D_DESC d={};d.Width=w;d.Height=h;d.MipLevels=mips;d.ArraySize=d.SampleDesc.Count=1;d.Format=(DXGI_FORMAT)format;d.Usage=D3D11_USAGE_IMMUTABLE;d.BindFlags=D3D11_BIND_SHADER_RESOURCE;
  std::vector<std::vector<char>>pixels(mips);std::vector<D3D11_SUBRESOURCE_DATA>sub(mips);
  for(UINT level=0;level<mips;level++){UINT mw=(w>>level)?(w>>level):1;pixels[level]=bytes(assets,read<uint32_t>(assets));sub[level]={};sub[level].pSysMem=pixels[level].data();sub[level].SysMemPitch=format<=29?mw*4:((mw+3)/4)*((format==71||format==72||format==80)?8:16);}
  ComPtr<ID3D11Texture2D>t;check(dev->CreateTexture2D(&d,sub.data(),&t));ComPtr<ID3D11ShaderResourceView>srv;check(dev->CreateShaderResourceView(t.Get(),nullptr,&srv));textures.push_back(srv);
 }
 std::vector<Shader>shaders;
 for(unsigned i=0;i<ns;i++){
  auto flags=read<uint32_t>(assets);auto a=bytes(assets,read<uint32_t>(assets));auto b=bytes(assets,read<uint32_t>(assets));Shader s;s.refraction=(flags&1)!=0;s.reentry=(flags&2)!=0;
  check(dev->CreateVertexShader(a.data(),a.size(),nullptr,&s.vs));check(dev->CreatePixelShader(b.data(),b.size(),nullptr,&s.ps));
   D3D11_INPUT_ELEMENT_DESC layout[]={{"POSITION",0,DXGI_FORMAT_R32G32B32_FLOAT,0,0,D3D11_INPUT_PER_VERTEX_DATA,0},
    {"TANGENT",0,DXGI_FORMAT_R32G32B32A32_FLOAT,0,0,D3D11_INPUT_PER_VERTEX_DATA,0},
    {"NORMAL",0,DXGI_FORMAT_R32G32B32_FLOAT,0,12,D3D11_INPUT_PER_VERTEX_DATA,0},
    {"TEXCOORD",0,DXGI_FORMAT_R32G32_FLOAT,0,24,D3D11_INPUT_PER_VERTEX_DATA,0},
    {"TEXCOORD",1,DXGI_FORMAT_R32G32_FLOAT,0,24,D3D11_INPUT_PER_VERTEX_DATA,0},
    {"TEXCOORD",2,DXGI_FORMAT_R32G32_FLOAT,0,24,D3D11_INPUT_PER_VERTEX_DATA,0},
    {"TEXCOORD",3,DXGI_FORMAT_R32G32_FLOAT,0,24,D3D11_INPUT_PER_VERTEX_DATA,0},
    {"COLOR",0,DXGI_FORMAT_R32G32B32A32_FLOAT,0,0,D3D11_INPUT_PER_VERTEX_DATA,0}};
  if(s.reentry)layout[7].AlignedByteOffset=32;
  check(dev->CreateInputLayout(layout,8,a.data(),a.size(),&s.layout));shaders.push_back(s);
 }
 std::vector<Mesh>meshes;
 for(unsigned i=0;i<nm;i++){
  Mesh m;m.vertices=read<uint32_t>(assets);m.indices=read<uint32_t>(assets);auto b=bytes(assets,size_t(m.vertices)*104);
  m.v.resize(size_t(m.vertices)*26);memcpy(m.v.data(),b.data(),b.size());auto ix=bytes(assets,size_t(m.indices)*4);m.ib=buffer(ix.data(),(UINT)ix.size(),D3D11_BIND_INDEX_BUFFER);meshes.push_back(std::move(m));
 }
 std::ifstream frame(argv[2],std::ios::binary);if(read<uint32_t>(frame)!=0x46525846)throw std::runtime_error("bad frame magic");
 UINT width=read<uint32_t>(frame),height=read<uint32_t>(frame),draws=read<uint32_t>(frame),grid=read<uint32_t>(frame);
 D3D11_TEXTURE2D_DESC target={};target.Width=width;target.Height=height;target.MipLevels=target.ArraySize=target.SampleDesc.Count=1;target.Format=DXGI_FORMAT_R16G16B16A16_FLOAT;target.Usage=D3D11_USAGE_DEFAULT;target.BindFlags=D3D11_BIND_RENDER_TARGET|D3D11_BIND_SHADER_RESOURCE;
 ComPtr<ID3D11Texture2D>color;check(dev->CreateTexture2D(&target,nullptr,&color));ComPtr<ID3D11RenderTargetView>rtv;check(dev->CreateRenderTargetView(color.Get(),nullptr,&rtv));
 target.Format=DXGI_FORMAT_R32_TYPELESS;target.BindFlags=D3D11_BIND_DEPTH_STENCIL|D3D11_BIND_SHADER_RESOURCE;ComPtr<ID3D11Texture2D>depth;check(dev->CreateTexture2D(&target,nullptr,&depth));
 D3D11_DEPTH_STENCIL_VIEW_DESC dd={};dd.Format=DXGI_FORMAT_D32_FLOAT;dd.ViewDimension=D3D11_DSV_DIMENSION_TEXTURE2D;ComPtr<ID3D11DepthStencilView>writeDepth,dsv;check(dev->CreateDepthStencilView(depth.Get(),&dd,&writeDepth));dd.Flags=D3D11_DSV_READ_ONLY_DEPTH;check(dev->CreateDepthStencilView(depth.Get(),&dd,&dsv));
 D3D11_SHADER_RESOURCE_VIEW_DESC depthDesc={};depthDesc.Format=DXGI_FORMAT_R32_FLOAT;depthDesc.ViewDimension=D3D11_SRV_DIMENSION_TEXTURE2D;depthDesc.Texture2D.MipLevels=1;ComPtr<ID3D11ShaderResourceView>depthSrv;check(dev->CreateShaderResourceView(depth.Get(),&depthDesc,&depthSrv));
 auto rt=rtv.Get();ctx->OMSetRenderTargets(1,&rt,dsv.Get());float clear[]={0,0,0,1};ctx->ClearRenderTargetView(rt,clear);ctx->ClearDepthStencilView(writeDepth.Get(),D3D11_CLEAR_DEPTH,0,0);
 if(grid){std::vector<uint16_t>pixels(size_t(width)*height*4);for(UINT y=0;y<height;y++)for(UINT x=0;x<width;x++){
  size_t i=(size_t(y)*width+x)*4;pixels[i]=grid_half(0.125f+0.25f*((x/16+y/16)%2));pixels[i+1]=grid_half(0.125f+0.5f*(y+0.5f)/height);pixels[i+2]=grid_half(0.125f+0.5f*(x+0.5f)/width);pixels[i+3]=grid_half(1.0f);}
  ctx->UpdateSubresource(color.Get(),0,nullptr,pixels.data(),width*8,0);}
 D3D11_TEXTURE2D_DESC sceneDesc; color->GetDesc(&sceneDesc);ComPtr<ID3D11Texture2D>scene;check(dev->CreateTexture2D(&sceneDesc,nullptr,&scene));ctx->CopyResource(scene.Get(),color.Get());ComPtr<ID3D11ShaderResourceView>sceneSrv;check(dev->CreateShaderResourceView(scene.Get(),nullptr,&sceneSrv));
 D3D11_VIEWPORT viewport={0,0,(float)width,(float)height,0,1};ctx->RSSetViewports(1,&viewport);
 D3D11_RASTERIZER_DESC rs={};rs.FillMode=D3D11_FILL_SOLID;rs.CullMode=D3D11_CULL_NONE;rs.FrontCounterClockwise=TRUE;rs.DepthClipEnable=TRUE;ComPtr<ID3D11RasterizerState>raster;check(dev->CreateRasterizerState(&rs,&raster));ctx->RSSetState(raster.Get());
 D3D11_DEPTH_STENCIL_DESC ds={};ds.DepthEnable=TRUE;ds.DepthWriteMask=D3D11_DEPTH_WRITE_MASK_ZERO;ds.DepthFunc=D3D11_COMPARISON_GREATER;ComPtr<ID3D11DepthStencilState>depthState;check(dev->CreateDepthStencilState(&ds,&depthState));ctx->OMSetDepthStencilState(depthState.Get(),0);
 D3D11_BLEND_DESC blend={};blend.RenderTarget[0].BlendEnable=TRUE;blend.RenderTarget[0].SrcBlend=D3D11_BLEND_SRC_ALPHA;blend.RenderTarget[0].DestBlend=D3D11_BLEND_ONE;blend.RenderTarget[0].BlendOp=D3D11_BLEND_OP_ADD;blend.RenderTarget[0].SrcBlendAlpha=D3D11_BLEND_ONE;blend.RenderTarget[0].DestBlendAlpha=D3D11_BLEND_INV_SRC_ALPHA;blend.RenderTarget[0].BlendOpAlpha=D3D11_BLEND_OP_ADD;blend.RenderTarget[0].RenderTargetWriteMask=D3D11_COLOR_WRITE_ENABLE_ALL;
 ComPtr<ID3D11BlendState>blendState;check(dev->CreateBlendState(&blend,&blendState));ctx->OMSetBlendState(blendState.Get(),nullptr,0xffffffff);
 blend.RenderTarget[0].BlendEnable=FALSE;ComPtr<ID3D11BlendState>opaqueState;check(dev->CreateBlendState(&blend,&opaqueState));
 D3D11_SAMPLER_DESC sd={};sd.Filter=D3D11_FILTER_MIN_MAG_LINEAR_MIP_POINT;sd.AddressU=sd.AddressV=sd.AddressW=D3D11_TEXTURE_ADDRESS_WRAP;sd.MaxLOD=D3D11_FLOAT32_MAX;ComPtr<ID3D11SamplerState>sampler;check(dev->CreateSamplerState(&sd,&sampler));ID3D11SamplerState*ss[]={sampler.Get(),sampler.Get()};ctx->VSSetSamplers(0,1,ss);ctx->PSSetSamplers(0,2,ss);
 sd.AddressU=sd.AddressV=sd.AddressW=D3D11_TEXTURE_ADDRESS_CLAMP;ComPtr<ID3D11SamplerState>screenSampler;check(dev->CreateSamplerState(&sd,&screenSampler));sd.Filter=D3D11_FILTER_MIN_MAG_MIP_POINT;ComPtr<ID3D11SamplerState>depthSampler;check(dev->CreateSamplerState(&sd,&depthSampler));
 ctx->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
 for(UINT draw=0;draw<draws;draw++){
  auto mi=read<uint32_t>(frame),si=read<uint32_t>(frame);std::array<uint32_t,3>ti;frame.read((char*)ti.data(),12);
  std::array<float,432>u;frame.read((char*)u.data(),sizeof(u));const auto&m=meshes.at(mi);const auto&s=shaders.at(si);
  auto floats=s.reentry?12:8;std::vector<float>v(size_t(m.vertices)*floats);
  for(unsigned i=0;i<m.vertices;i++){
   const float*src=&m.v[size_t(i)*26];float*dst=&v[size_t(i)*floats];memcpy(dst,src,32);
   if(s.reentry){memcpy(dst+8,src+8,12);dst[11]=1;}
   if(!s.reentry){for(int shape=0;shape<3;shape++)for(int axis=0;axis<3;axis++){dst[axis]+=src[8+shape*6+axis]*u[428+shape];dst[3+axis]+=src[11+shape*6+axis]*u[428+shape];}
   float norm=sqrtf(dst[3]*dst[3]+dst[4]*dst[4]+dst[5]*dst[5]);for(int a=3;a<6;a++)dst[a]/=norm;}
  }
  auto vb=buffer(v.data(),(UINT)v.size()*4,D3D11_BIND_VERTEX_BUFFER);UINT stride=floats*4,offset=0;auto raw=vb.Get();ctx->IASetVertexBuffers(0,1,&raw,&stride,&offset);ctx->IASetIndexBuffer(m.ib.Get(),DXGI_FORMAT_R32_UINT,0);ctx->IASetInputLayout(s.layout.Get());ctx->VSSetShader(s.vs.Get(),nullptr,0);ctx->PSSetShader(s.ps.Get(),nullptr,0);
  UINT sizes[]={64*16,9*16,11*16,23*16};std::vector<ComPtr<ID3D11Buffer>>buffers;std::vector<ID3D11Buffer*>raws;size_t at=0;
  for(auto n:sizes){buffers.push_back(buffer((char*)u.data()+at,n,D3D11_BIND_CONSTANT_BUFFER));raws.push_back(buffers.back().Get());at+=n;}
  ctx->VSSetConstantBuffers(0,4,raws.data());ctx->PSSetConstantBuffers(0,3,raws.data());ID3D11ShaderResourceView*vsmap[]={textures.at(ti[0]).Get()};ctx->VSSetShaderResources(0,1,vsmap);
  std::vector<ComPtr<ID3D11Buffer>>structured;std::vector<ComPtr<ID3D11ShaderResourceView>>structuredViews;ComPtr<ID3D11Texture2D>occlusion;ComPtr<ID3D11ShaderResourceView>occlusionSrv;
  if(s.reentry){
   std::array<float,68>extra;frame.read((char*)extra.data(),sizeof(extra));
   for(auto pair:std::array<std::pair<UINT,UINT>,2>{{{0,156},{40,112}}}){
    D3D11_BUFFER_DESC bd={};bd.ByteWidth=bd.StructureByteStride=pair.second;bd.Usage=D3D11_USAGE_DEFAULT;bd.BindFlags=D3D11_BIND_SHADER_RESOURCE;bd.MiscFlags=D3D11_RESOURCE_MISC_BUFFER_STRUCTURED;
    D3D11_SUBRESOURCE_DATA initial={};initial.pSysMem=extra.data()+pair.first;ComPtr<ID3D11Buffer>buf;check(dev->CreateBuffer(&bd,&initial,&buf));
    D3D11_SHADER_RESOURCE_VIEW_DESC sv={};sv.ViewDimension=D3D11_SRV_DIMENSION_BUFFER;sv.Buffer.NumElements=1;ComPtr<ID3D11ShaderResourceView>view;check(dev->CreateShaderResourceView(buf.Get(),&sv,&view));structured.push_back(buf);structuredViews.push_back(view);
   }
   auto pixels=bytes(frame,2048*2048*2);D3D11_TEXTURE2D_DESC td={};td.Width=td.Height=2048;td.MipLevels=td.ArraySize=td.SampleDesc.Count=1;td.Format=DXGI_FORMAT_R16_FLOAT;td.Usage=D3D11_USAGE_IMMUTABLE;td.BindFlags=D3D11_BIND_SHADER_RESOURCE;
   D3D11_SUBRESOURCE_DATA initial={};initial.pSysMem=pixels.data();initial.SysMemPitch=4096;check(dev->CreateTexture2D(&td,&initial,&occlusion));check(dev->CreateShaderResourceView(occlusion.Get(),nullptr,&occlusionSrv));
   ID3D11ShaderResourceView*vm[]={structuredViews[0].Get()};ctx->VSSetShaderResources(0,1,vm);
   ID3D11ShaderResourceView*pm[]={occlusionSrv.Get(),textures.at(ti[1]).Get(),structuredViews[0].Get(),structuredViews[1].Get()};ctx->PSSetShaderResources(0,4,pm);
   ID3D11SamplerState*samplers[]={screenSampler.Get(),depthSampler.Get()};ctx->PSSetSamplers(0,2,samplers);ctx->OMSetBlendState(blendState.Get(),nullptr,0xffffffff);
  }
  else if(s.refraction){ID3D11ShaderResourceView*map[]={textures.at(ti[0]).Get(),depthSrv.Get(),sceneSrv.Get()};ID3D11SamplerState*samplers[]={sampler.Get(),screenSampler.Get(),depthSampler.Get()};ctx->PSSetShaderResources(0,3,map);ctx->PSSetSamplers(0,3,samplers);ctx->OMSetBlendState(opaqueState.Get(),nullptr,0xffffffff);}
  else{ID3D11ShaderResourceView*map[]={textures.at(ti[1]).Get(),textures.at(ti[2]).Get(),nullptr};ctx->PSSetShaderResources(0,3,map);ctx->PSSetSamplers(0,2,ss);ctx->OMSetBlendState(blendState.Get(),nullptr,0xffffffff);}
  ctx->DrawIndexed(m.indices,0,0);
 }
 color->GetDesc(&target);target.Usage=D3D11_USAGE_STAGING;target.BindFlags=0;target.CPUAccessFlags=D3D11_CPU_ACCESS_READ;ComPtr<ID3D11Texture2D>staging;check(dev->CreateTexture2D(&target,nullptr,&staging));ctx->CopyResource(staging.Get(),color.Get());D3D11_MAPPED_SUBRESOURCE mapped;check(ctx->Map(staging.Get(),0,D3D11_MAP_READ,0,&mapped));
 std::ofstream output(argv[3],std::ios::binary);for(UINT y=0;y<height;y++)output.write((char*)mapped.pData+y*mapped.RowPitch,width*8);ctx->Unmap(staging.Get(),0);
 return 0;
}catch(const std::exception&e){std::cerr<<e.what()<<"\n";return 1;}}
