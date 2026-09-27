use std::{collections::HashMap, mem::ManuallyDrop};

use anyhow::{anyhow, Context, Result};
use windows::{
    core::{Interface, PCSTR},
    Win32::{
        Foundation::RECT,
        Graphics::{
            Direct3D::{
                Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3},
                ID3DBlob, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
            },
            Direct3D11::{
                D3D11_VIDEO_COLOR_YCbCrA, ID3D11Multithread, ID3D11PixelShader,
                ID3D11RenderTargetView, ID3D11ShaderResourceView, ID3D11Texture2D,
                ID3D11VertexShader, ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor,
                ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
                ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET,
                D3D11_BIND_SHADER_RESOURCE, D3D11_SUBRESOURCE_DATA, D3D11_TEX2D_ARRAY_VPOV,
                D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
                D3D11_VIDEO_COLOR, D3D11_VIDEO_COLOR_0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                D3D11_VIDEO_PROCESSOR_COLOR_SPACE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
                D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
                D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
                D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VIEWPORT,
                D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
                D3D11_VPOV_DIMENSION_TEXTURE2DARRAY,
            },
            Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_RATIONAL, DXGI_SAMPLE_DESC},
        },
    },
};

use super::device::Gpu;
use crate::layout::fit;

const OPAQUE: &str = "Texture2D<float4> t : register(t0);\n\
float4 vs(uint id : SV_VertexID) : SV_Position {\n\
    float2 uv = float2((id << 1) & 2, id & 2);\n\
    return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);\n\
}\n\
float4 ps(float4 p : SV_Position) : SV_Target {\n\
    return float4(t.Load(int3(p.xy, 0)).rgb, 1);\n\
}\n";

const RGB_FULL: u32 = 0;
const YUV_709_STUDIO: u32 = (1 << 2) | (1 << 4);
const BLACK_SIZE: u32 = 16;

pub struct Converter {
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    output: (u32, u32),
    fps: u32,
    input: (u32, u32),
    processor: Option<(ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor)>,
    input_view: Option<(usize, ID3D11VideoProcessorInputView)>,
    output_views: HashMap<(usize, u32), ID3D11VideoProcessorOutputView>,
    black: ID3D11Texture2D,
    opaque: Opaque,
}

fn compile(entry: &str, target: &str) -> Result<Vec<u8>> {
    let entry = std::ffi::CString::new(entry)?;
    let target = std::ffi::CString::new(target)?;
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let result = unsafe {
        D3DCompile(
            OPAQUE.as_ptr() as *const _,
            OPAQUE.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry.as_ptr() as *const u8),
            PCSTR(target.as_ptr() as *const u8),
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(e) = result {
        let detail = errors
            .map(|b| unsafe {
                String::from_utf8_lossy(std::slice::from_raw_parts(
                    b.GetBufferPointer() as *const u8,
                    b.GetBufferSize(),
                ))
                .into_owned()
            })
            .unwrap_or_default();
        return Err(anyhow!("compilazione dello shader: {e} {detail}"));
    }
    let code = code.context("shader vuoto")?;
    Ok(unsafe {
        std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, code.GetBufferSize())
    }
    .to_vec())
}

struct Opaque {
    gpu: Gpu,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    lock: Option<ID3D11Multithread>,
    source: Option<(usize, ID3D11ShaderResourceView)>,
    target: Option<(ID3D11Texture2D, ID3D11RenderTargetView, (u32, u32))>,
}

impl Opaque {
    fn new(gpu: &Gpu) -> Result<Self> {
        let vs_code = compile("vs", "vs_4_0")?;
        let ps_code = compile("ps", "ps_4_0")?;
        let mut vs = None;
        let mut ps = None;
        unsafe {
            gpu.device
                .CreateVertexShader(&vs_code, None, Some(&mut vs))?;
            gpu.device
                .CreatePixelShader(&ps_code, None, Some(&mut ps))?;
        }
        Ok(Self {
            gpu: gpu.clone(),
            vs: vs.context("vertex shader mancante")?,
            ps: ps.context("pixel shader mancante")?,
            lock: gpu.context.cast().ok(),
            source: None,
            target: None,
        })
    }

    fn apply(&mut self, source: &ID3D11Texture2D, size: (u32, u32)) -> Result<ID3D11Texture2D> {
        if self.target.as_ref().is_none_or(|t| t.2 != size) {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: size.0,
                Height: size.1,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut texture = None;
            unsafe {
                self.gpu
                    .device
                    .CreateTexture2D(&desc, None, Some(&mut texture))
            }?;
            let texture = texture.context("immagine opaca mancante")?;
            let mut view = None;
            unsafe {
                self.gpu
                    .device
                    .CreateRenderTargetView(&texture, None, Some(&mut view))
            }?;
            self.target = Some((texture, view.context("vista opaca mancante")?, size));
        }
        let key = source.as_raw() as usize;
        if self.source.as_ref().is_none_or(|s| s.0 != key) {
            let mut view = None;
            unsafe {
                self.gpu
                    .device
                    .CreateShaderResourceView(source, None, Some(&mut view))
            }?;
            self.source = Some((key, view.context("vista della cattura mancante")?));
        }
        let (texture, target, _) = self.target.as_ref().unwrap();
        let input = self.source.as_ref().unwrap().1.clone();
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: size.0 as f32,
            Height: size.1 as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        unsafe {
            let c = &self.gpu.context;
            if let Some(l) = &self.lock {
                l.Enter();
            }
            c.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            c.RSSetViewports(Some(&[viewport]));
            c.IASetInputLayout(None);
            c.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vs, None);
            c.PSSetShader(&self.ps, None);
            c.PSSetShaderResources(0, Some(&[Some(input)]));
            c.OMSetBlendState(None, None, 0xFFFF_FFFF);
            c.Draw(3, 0);
            c.PSSetShaderResources(0, Some(&[None]));
            c.OMSetRenderTargets(None, None);
            if let Some(l) = &self.lock {
                l.Leave();
            }
        }
        Ok(texture.clone())
    }
}

impl Converter {
    pub fn new(gpu: &Gpu, output: (u32, u32), fps: u32) -> Result<Self> {
        let video_device: ID3D11VideoDevice = gpu
            .device
            .cast()
            .context("la scheda video non ha il processore video di Direct3D 11")?;
        let video_context: ID3D11VideoContext = gpu.context.cast()?;
        let pixels = vec![0u8; (BLACK_SIZE * BLACK_SIZE * 4) as usize];
        let init = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr() as *const _,
            SysMemPitch: BLACK_SIZE * 4,
            SysMemSlicePitch: 0,
        };
        let desc = D3D11_TEXTURE2D_DESC {
            Width: BLACK_SIZE,
            Height: BLACK_SIZE,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut black = None;
        unsafe {
            gpu.device
                .CreateTexture2D(&desc, Some(&init), Some(&mut black))
        }?;
        Ok(Self {
            video_device,
            video_context,
            output,
            fps,
            input: (0, 0),
            processor: None,
            input_view: None,
            output_views: HashMap::new(),
            black: black.context("texture nera mancante")?,
            opaque: Opaque::new(gpu)?,
        })
    }

    pub fn resize(&mut self, output: (u32, u32)) {
        self.output = output;
        self.processor = None;
        self.input_view = None;
        self.output_views.clear();
    }

    fn ensure(&mut self, input: (u32, u32)) -> Result<()> {
        if self.processor.is_some() && self.input == input {
            return Ok(());
        }
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: self.fps,
                Denominator: 1,
            },
            InputWidth: input.0,
            InputHeight: input.1,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: self.fps,
                Denominator: 1,
            },
            OutputWidth: self.output.0,
            OutputHeight: self.output.1,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        let enumerator = unsafe { self.video_device.CreateVideoProcessorEnumerator(&desc) }
            .context("processore video non disponibile")?;
        let processor = unsafe { self.video_device.CreateVideoProcessor(&enumerator, 0) }
            .context("creazione del processore video")?;
        let background = D3D11_VIDEO_COLOR {
            Anonymous: D3D11_VIDEO_COLOR_0 {
                YCbCr: D3D11_VIDEO_COLOR_YCbCrA {
                    Y: 16.0 / 255.0,
                    Cb: 0.5,
                    Cr: 0.5,
                    A: 1.0,
                },
            },
        };
        unsafe {
            let c = &self.video_context;
            c.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            c.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            c.VideoProcessorSetStreamColorSpace(
                &processor,
                0,
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                    _bitfield: RGB_FULL,
                },
            );
            c.VideoProcessorSetOutputColorSpace(
                &processor,
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                    _bitfield: YUV_709_STUDIO,
                },
            );
            c.VideoProcessorSetOutputBackgroundColor(&processor, true, &background);
        }
        self.processor = Some((enumerator, processor));
        self.input = input;
        self.input_view = None;
        self.output_views.clear();
        Ok(())
    }

    fn input_view(&mut self, texture: &ID3D11Texture2D) -> Result<ID3D11VideoProcessorInputView> {
        let key = texture.as_raw() as usize;
        if let Some((k, v)) = &self.input_view {
            if *k == key {
                return Ok(v.clone());
            }
        }
        let (enumerator, _) = self.processor.as_ref().unwrap();
        let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut view = None;
        unsafe {
            self.video_device.CreateVideoProcessorInputView(
                texture,
                enumerator,
                &desc,
                Some(&mut view),
            )
        }
        .context("vista d'ingresso del processore video")?;
        let view = view.context("vista d'ingresso mancante")?;
        self.input_view = Some((key, view.clone()));
        Ok(view)
    }

    fn output_view(
        &mut self,
        target: &ID3D11Texture2D,
        slice: u32,
    ) -> Result<ID3D11VideoProcessorOutputView> {
        let key = (target.as_raw() as usize, slice);
        if let Some(v) = self.output_views.get(&key) {
            return Ok(v.clone());
        }
        let mut tdesc = D3D11_TEXTURE2D_DESC::default();
        unsafe { target.GetDesc(&mut tdesc) };
        let desc = if tdesc.ArraySize > 1 {
            D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2DARRAY,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2DArray: D3D11_TEX2D_ARRAY_VPOV {
                        MipSlice: 0,
                        FirstArraySlice: slice,
                        ArraySize: 1,
                    },
                },
            }
        } else {
            D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            }
        };
        let (enumerator, _) = self.processor.as_ref().unwrap();
        let mut view = None;
        unsafe {
            self.video_device.CreateVideoProcessorOutputView(
                target,
                enumerator,
                &desc,
                Some(&mut view),
            )
        }
        .context("vista d'uscita del processore video")?;
        let view = view.context("vista d'uscita mancante")?;
        if self.output_views.len() > 64 {
            self.output_views.clear();
        }
        self.output_views.insert(key, view.clone());
        Ok(view)
    }

    pub fn convert(
        &mut self,
        source: Option<(&ID3D11Texture2D, (u32, u32))>,
        target: &ID3D11Texture2D,
        slice: u32,
    ) -> Result<()> {
        let black = self.black.clone();
        let (texture, size, dest) = match source {
            Some((t, s)) => (self.opaque.apply(t, s)?, s, fit(s, self.output)),
            None => (
                black,
                (BLACK_SIZE, BLACK_SIZE),
                crate::layout::Rect {
                    x: 0,
                    y: 0,
                    width: self.output.0,
                    height: self.output.1,
                },
            ),
        };
        self.ensure(size)?;
        let input = self.input_view(&texture)?;
        let output = self.output_view(target, slice)?;
        let processor = self.processor.as_ref().unwrap().1.clone();
        let source_rect = RECT {
            left: 0,
            top: 0,
            right: size.0 as i32,
            bottom: size.1 as i32,
        };
        let dest_rect = RECT {
            left: dest.x as i32,
            top: dest.y as i32,
            right: (dest.x + dest.width) as i32,
            bottom: (dest.y + dest.height) as i32,
        };
        let target_rect = RECT {
            left: 0,
            top: 0,
            right: self.output.0 as i32,
            bottom: self.output.1 as i32,
        };
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(input)),
            ..Default::default()
        };
        let mut streams = [stream];
        let result = unsafe {
            let c = &self.video_context;
            c.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&source_rect));
            c.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dest_rect));
            c.VideoProcessorSetOutputTargetRect(&processor, true, Some(&target_rect));
            c.VideoProcessorBlt(&processor, &output, 0, &streams)
        };
        unsafe { ManuallyDrop::drop(&mut streams[0].pInputSurface) };
        result.context("conversione del fotogramma sulla scheda video")
    }
}
