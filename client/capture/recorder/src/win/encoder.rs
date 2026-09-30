use super::timing::{self, Stage};
use std::{
    collections::VecDeque,
    mem::ManuallyDrop,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use windows::{
    core::{Interface, GUID, PWSTR},
    Win32::{
        Foundation::LUID,
        Foundation::{VARIANT_FALSE, VARIANT_TRUE},
        Graphics::{
            Direct3D11::{
                ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_VIDEO_ENCODER,
                D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
                D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
            },
            Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC},
        },
        Media::MediaFoundation::*,
        System::{
            Com::CoTaskMemFree,
            Variant::{VARIANT, VT_BOOL, VT_UI4},
        },
    },
};

use super::device::{Gpu, VENDOR_AMD, VENDOR_INTEL, VENDOR_NVIDIA};
use crate::EncoderPref;

pub fn startup() {
    unsafe {
        let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Nvenc,
    Amf,
    Qsv,
    OtherHardware,
    Software,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Nvenc => "nvenc",
            Kind::Amf => "amf",
            Kind::Qsv => "qsv",
            Kind::OtherHardware => "hardware",
            Kind::Software => "software",
        }
    }

    pub fn matches(self, pref: EncoderPref) -> bool {
        match pref {
            EncoderPref::Auto => true,
            EncoderPref::Nvenc => self == Kind::Nvenc,
            EncoderPref::Amf => self == Kind::Amf,
            EncoderPref::Qsv => self == Kind::Qsv,
            EncoderPref::Software => self == Kind::Software,
        }
    }
}

#[derive(Clone)]
pub struct Candidate {
    pub activate: IMFActivate,
    pub kind: Kind,
    pub label: String,
}

fn allocated_string(attrs: &IMFAttributes, key: &GUID) -> Option<String> {
    unsafe {
        let mut ptr = PWSTR::null();
        let mut len = 0u32;
        attrs.GetAllocatedString(key, &mut ptr, &mut len).ok()?;
        let s = ptr.to_string().ok();
        CoTaskMemFree(Some(ptr.0 as *const _));
        s
    }
}

fn collect(ptr: *mut Option<IMFActivate>, count: u32) -> Vec<IMFActivate> {
    if ptr.is_null() {
        return Vec::new();
    }
    let mut out = Vec::new();
    unsafe {
        let items = std::slice::from_raw_parts_mut(ptr, count as usize);
        for item in items.iter_mut() {
            if let Some(a) = item.take() {
                out.push(a);
            }
        }
        CoTaskMemFree(Some(ptr as *const _));
    }
    out
}

fn kind_of(activate: &IMFActivate, fallback_vendor: Option<u32>) -> Kind {
    let vendor = allocated_string(activate, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute)
        .and_then(|v| u32::from_str_radix(v.trim_start_matches("VEN_"), 16).ok())
        .or(fallback_vendor);
    match vendor {
        Some(VENDOR_NVIDIA) => Kind::Nvenc,
        Some(VENDOR_AMD) => Kind::Amf,
        Some(VENDOR_INTEL) => Kind::Qsv,
        _ => Kind::OtherHardware,
    }
}

pub fn hardware(luid: LUID, vendor: u32) -> Vec<Candidate> {
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut attrs = None;
    if unsafe { MFCreateAttributes(&mut attrs, 1) }.is_err() {
        return Vec::new();
    }
    let Some(attrs) = attrs else {
        return Vec::new();
    };
    let bytes: [u8; 8] = unsafe { std::mem::transmute(luid) };
    if unsafe { attrs.SetBlob(&MFT_ENUM_ADAPTER_LUID, &bytes) }.is_err() {
        return Vec::new();
    }
    let mut ptr = std::ptr::null_mut();
    let mut count = 0u32;
    let found = unsafe {
        MFTEnum2(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            None,
            Some(&output),
            &attrs,
            &mut ptr,
            &mut count,
        )
    };
    if found.is_err() {
        return Vec::new();
    }
    collect(ptr, count)
        .into_iter()
        .map(|a| Candidate {
            kind: kind_of(&a, Some(vendor)),
            label: allocated_string(&a, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_default(),
            activate: a,
        })
        .collect()
}

pub fn software() -> Vec<Candidate> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut ptr = std::ptr::null_mut();
    let mut count = 0u32;
    let found = unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut ptr,
            &mut count,
        )
    };
    if found.is_err() {
        return Vec::new();
    }
    collect(ptr, count)
        .into_iter()
        .map(|a| Candidate {
            kind: Kind::Software,
            label: allocated_string(&a, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_default(),
            activate: a,
        })
        .collect()
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

fn uint(v: u32) -> VARIANT {
    let mut var = VARIANT::default();
    unsafe {
        let inner = &mut *var.Anonymous.Anonymous;
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = v;
    }
    var
}

fn flag(v: bool) -> VARIANT {
    let mut var = VARIANT::default();
    unsafe {
        let inner = &mut *var.Anonymous.Anonymous;
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = if v { VARIANT_TRUE } else { VARIANT_FALSE };
    }
    var
}

fn set_codec(codec: &ICodecAPI, key: &GUID, value: VARIANT) -> bool {
    unsafe { codec.SetValue(key, &value) }.is_ok()
}

fn configure(codec: &ICodecAPI, kbps: u32, gop: u32) {
    set_codec(
        codec,
        &CODECAPI_AVEncCommonRateControlMode,
        uint(eAVEncCommonRateControlMode_CBR.0 as u32),
    );
    set_codec(codec, &CODECAPI_AVEncCommonMeanBitRate, uint(kbps * 1000));
    set_codec(codec, &CODECAPI_AVEncMPVGOPSize, uint(gop));
    set_codec(codec, &CODECAPI_AVEncMPVDefaultBPictureCount, uint(0));
    set_codec(codec, &CODECAPI_AVLowLatencyMode, flag(true));
    set_codec(codec, &CODECAPI_AVEncCommonQualityVsSpeed, uint(60));
}

fn color(t: &IMFMediaType) {
    unsafe {
        let _ = t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32);
        let _ = t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32);
        let _ = t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
        let _ = t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
    }
}

pub struct Encoded {
    pub time: Option<i64>,
    pub data: Vec<u8>,
}

enum Input {
    Gpu(IMFVideoSampleAllocatorEx),
    Memory {
        target: ID3D11Texture2D,
        staging: ID3D11Texture2D,
    },
}

pub struct Frame {
    pub sample: IMFSample,
    pub texture: ID3D11Texture2D,
    pub slice: u32,
}

pub struct H264 {
    activate: IMFActivate,
    mft: IMFTransform,
    events: Option<IMFMediaEventGenerator>,
    codec: Option<ICodecAPI>,
    input_id: u32,
    output_id: u32,
    provides_samples: bool,
    output_size: u32,
    need_input: u32,
    pending: VecDeque<(IMFSample, bool)>,
    drained: bool,
    input: Input,
    gpu: Gpu,
    pub size: (u32, u32),
    pub sequence_header: Option<Vec<u8>>,
}

impl H264 {
    pub fn open(
        candidate: &Candidate,
        gpu: &Gpu,
        size: (u32, u32),
        fps: u32,
        kbps: u32,
        gop: u32,
    ) -> Result<Self> {
        let mft: IMFTransform = unsafe { candidate.activate.ActivateObject() }
            .with_context(|| format!("avvio dell'encoder {}", candidate.label))?;
        let attrs = unsafe { mft.GetAttributes() }.ok();
        let is_async = attrs
            .as_ref()
            .and_then(|a| unsafe { a.GetUINT32(&MF_TRANSFORM_ASYNC) }.ok())
            == Some(1);
        if is_async {
            if let Some(a) = &attrs {
                unsafe { a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1) }
                    .context("sblocco dell'encoder")?;
            }
        }
        let d3d_aware = attrs
            .as_ref()
            .and_then(|a| unsafe { a.GetUINT32(&MF_SA_D3D11_AWARE) }.ok())
            == Some(1);
        if candidate.kind != Kind::Software && !d3d_aware {
            let _ = unsafe { candidate.activate.ShutdownObject() };
            bail!("l'encoder hardware non accetta texture D3D11: escluso per evitare copie attraverso la CPU");
        }
        if d3d_aware {
            unsafe {
                mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, gpu.manager.as_raw() as usize)
            }
            .context("collegamento dell'encoder alla scheda video")?;
        }
        if let Some(a) = &attrs {
            let _ = unsafe { a.SetUINT32(&MF_LOW_LATENCY, 1) };
        }
        let mut ins = [0u32];
        let mut outs = [0u32];
        let (input_id, output_id) = match unsafe { mft.GetStreamIDs(&mut ins, &mut outs) } {
            Ok(()) => (ins[0], outs[0]),
            Err(_) => (0, 0),
        };

        let codec: Option<ICodecAPI> = mft.cast().ok();
        if let Some(c) = &codec {
            configure(c, kbps, gop);
        }

        let out_type = unsafe { MFCreateMediaType() }?;
        unsafe {
            out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out_type.SetUINT32(&MF_MT_AVG_BITRATE, kbps * 1000)?;
            out_type.SetUINT64(&MF_MT_FRAME_SIZE, pack(size.0, size.1))?;
            out_type.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            out_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            out_type.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
        }
        color(&out_type);
        unsafe { mft.SetOutputType(output_id, &out_type, 0) }
            .context("l'encoder non accetta H.264 con queste impostazioni")?;

        let mut in_type = None;
        for i in 0..64 {
            let Ok(t) = (unsafe { mft.GetInputAvailableType(input_id, i) }) else {
                break;
            };
            if unsafe { t.GetGUID(&MF_MT_SUBTYPE) }.ok() == Some(MFVideoFormat_NV12) {
                in_type = Some(t);
                break;
            }
        }
        let in_type = match in_type {
            Some(t) => t,
            None => {
                let t = unsafe { MFCreateMediaType() }?;
                unsafe {
                    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
                }
                t
            }
        };
        unsafe {
            in_type.SetUINT64(&MF_MT_FRAME_SIZE, pack(size.0, size.1))?;
            in_type.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            in_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        }
        color(&in_type);
        unsafe { mft.SetInputType(input_id, &in_type, 0) }
            .context("l'encoder non accetta immagini NV12")?;
        if let Some(c) = &codec {
            configure(c, kbps, gop);
        }

        let info = unsafe { mft.GetOutputStreamInfo(output_id) }?;
        let provides_samples = info.dwFlags
            & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0)
                as u32
            != 0;
        let events = if is_async {
            Some(
                mft.cast::<IMFMediaEventGenerator>()
                    .context("encoder asincrono senza eventi")?,
            )
        } else {
            None
        };
        let input = if d3d_aware {
            Input::Gpu(allocator(gpu, size)?)
        } else {
            Input::Memory {
                target: texture(gpu, size, false)?,
                staging: texture(gpu, size, true)?,
            }
        };

        unsafe {
            let _ = mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        }
        let mut enc = Self {
            activate: candidate.activate.clone(),
            mft,
            events,
            codec,
            input_id,
            output_id,
            provides_samples,
            output_size: info.cbSize.max(size.0 * size.1 * 3 / 2).max(1 << 20),
            need_input: 0,
            pending: VecDeque::new(),
            drained: false,
            input,
            gpu: gpu.clone(),
            size,
            sequence_header: None,
        };
        enc.read_sequence_header();
        Ok(enc)
    }

    fn read_sequence_header(&mut self) {
        let Ok(t) = (unsafe { self.mft.GetOutputCurrentType(self.output_id) }) else {
            return;
        };
        unsafe {
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            if t.GetAllocatedBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut ptr, &mut len)
                .is_ok()
                && !ptr.is_null()
            {
                self.sequence_header = Some(std::slice::from_raw_parts(ptr, len as usize).to_vec());
                CoTaskMemFree(Some(ptr as *const _));
            }
        }
    }

    pub fn frame(&mut self) -> Result<Option<Frame>> {
        let _measure = timing::span(Stage::Allocate);
        match &self.input {
            Input::Gpu(allocator) => {
                let sample = match unsafe { allocator.AllocateSample() } {
                    Ok(s) => s,
                    Err(e) if e.code() == MF_E_SAMPLEALLOCATOR_EMPTY => return Ok(None),
                    Err(e) => return Err(e).context("immagine per l'encoder"),
                };
                let buffer = unsafe { sample.GetBufferByIndex(0) }?;
                let dxgi: IMFDXGIBuffer = buffer.cast()?;
                let mut raw = std::ptr::null_mut();
                unsafe { dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw) }?;
                let texture = unsafe { ID3D11Texture2D::from_raw(raw) };
                let slice = unsafe { dxgi.GetSubresourceIndex() }.unwrap_or(0);
                Ok(Some(Frame {
                    sample,
                    texture,
                    slice,
                }))
            }
            Input::Memory { target, .. } => {
                let sample = unsafe { MFCreateSample() }?;
                Ok(Some(Frame {
                    sample,
                    texture: target.clone(),
                    slice: 0,
                }))
            }
        }
    }

    fn fill_memory(&self, sample: &IMFSample) -> Result<()> {
        let Input::Memory { target, staging } = &self.input else {
            return Ok(());
        };
        let (w, h) = (self.size.0 as usize, self.size.1 as usize);
        let total = w * h * 3 / 2;
        let buffer = unsafe { MFCreateMemoryBuffer(total as u32) }?;
        unsafe {
            self.gpu.context.CopyResource(staging, target);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.gpu
                .context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let mut dst = std::ptr::null_mut();
            let copied = buffer.Lock(&mut dst, None, None).map(|_| {
                let src = mapped.pData as *const u8;
                let pitch = mapped.RowPitch as usize;
                for row in 0..h {
                    std::ptr::copy_nonoverlapping(src.add(row * pitch), dst.add(row * w), w);
                }
                let chroma = src.add(pitch * h);
                for row in 0..h / 2 {
                    std::ptr::copy_nonoverlapping(
                        chroma.add(row * pitch),
                        dst.add(w * h + row * w),
                        w,
                    );
                }
            });
            self.gpu.context.Unmap(staging, 0);
            copied?;
            buffer.Unlock()?;
            buffer.SetCurrentLength(total as u32)?;
            sample.AddBuffer(&buffer)?;
        }
        Ok(())
    }

    pub fn encode(
        &mut self,
        frame: Frame,
        time: i64,
        duration: i64,
        keyframe: bool,
        out: &mut Vec<Encoded>,
    ) -> Result<()> {
        let sample = frame.sample;
        if matches!(self.input, Input::Memory { .. }) {
            self.fill_memory(&sample)?;
        } else {
            let buffer = unsafe { sample.GetBufferByIndex(0) }?;
            if let Ok(two) = buffer.cast::<IMF2DBuffer>() {
                if let Ok(len) = unsafe { two.GetContiguousLength() } {
                    let _ = unsafe { buffer.SetCurrentLength(len) };
                }
            }
        }
        unsafe {
            sample.SetSampleTime(time)?;
            sample.SetSampleDuration(duration)?;
        }
        if self.events.is_some() {
            self.pending.push_back((sample, keyframe));
            self.poll(out)
        } else {
            self.force_keyframe(keyframe);
            let mut tries = 0;
            loop {
                match { let _measure = timing::span(Stage::Input); unsafe { self.mft.ProcessInput(self.input_id, &sample, 0) } } {
                    Ok(()) => break,
                    Err(e) if e.code() == MF_E_NOTACCEPTING && tries < 4 => {
                        tries += 1;
                        self.pull(out)?;
                    }
                    Err(e) => return Err(e).context("invio del fotogramma all'encoder"),
                }
            }
            self.pull(out)
        }
    }

    fn force_keyframe(&self, keyframe: bool) {
        if keyframe {
            if let Some(c) = &self.codec {
                set_codec(c, &CODECAPI_AVEncVideoForceKeyFrame, uint(1));
            }
        }
    }

    fn pull(&mut self, out: &mut Vec<Encoded>) -> Result<()> {
        while let Some(e) = self.output()? {
            out.push(e);
        }
        Ok(())
    }

    pub fn poll(&mut self, out: &mut Vec<Encoded>) -> Result<()> {
        let _measure = timing::span(Stage::Poll);
        let Some(events) = self.events.clone() else {
            return Ok(());
        };
        loop {
            self.feed()?;
            let event = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(e) => e,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => break,
                Err(e) => return Err(e).context("eventi dell'encoder"),
            };
            let kind = MF_EVENT_TYPE(unsafe { event.GetType() }? as i32);
            if kind == METransformNeedInput {
                self.need_input += 1;
            } else if kind == METransformHaveOutput {
                if let Some(e) = self.output()? {
                    out.push(e);
                }
            } else if kind == METransformDrainComplete {
                self.drained = true;
            }
        }
        self.feed()
    }

    fn feed(&mut self) -> Result<()> {
        while self.need_input > 0 {
            let Some((sample, keyframe)) = self.pending.pop_front() else {
                break;
            };
            self.force_keyframe(keyframe);
            { let _measure = timing::span(Stage::Input); unsafe { self.mft.ProcessInput(self.input_id, &sample, 0) } }
                .context("invio del fotogramma all'encoder")?;
            self.need_input -= 1;
        }
        Ok(())
    }

    fn output(&mut self) -> Result<Option<Encoded>> {
        let sample = if self.provides_samples {
            None
        } else {
            let s = unsafe { MFCreateSample() }?;
            let b = unsafe { MFCreateMemoryBuffer(self.output_size) }?;
            unsafe { s.AddBuffer(&b) }?;
            Some(s)
        };
        let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: self.output_id,
            pSample: ManuallyDrop::new(sample),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        }];
        let mut status = 0u32;
        let result = { let _measure = timing::span(Stage::Output); unsafe { self.mft.ProcessOutput(0, &mut buffers, &mut status) } };
        let sample = unsafe { ManuallyDrop::take(&mut buffers[0].pSample) };
        drop(unsafe { ManuallyDrop::take(&mut buffers[0].pEvents) });
        match result {
            Ok(()) => {}
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                let t = unsafe { self.mft.GetOutputAvailableType(self.output_id, 0) }?;
                unsafe { self.mft.SetOutputType(self.output_id, &t, 0) }?;
                self.read_sequence_header();
                let info = unsafe { self.mft.GetOutputStreamInfo(self.output_id) }?;
                self.output_size = self.output_size.max(info.cbSize);
                return Ok(None);
            }
            Err(e) => return Err(e).context("lettura dall'encoder"),
        }
        let sample = sample.ok_or_else(|| anyhow!("l'encoder non ha restituito dati"))?;
        let time = unsafe { sample.GetSampleTime() }.ok();
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }?;
        let data = unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec();
        unsafe { buffer.Unlock() }?;
        if data.is_empty() {
            return Ok(None);
        }
        Ok(Some(Encoded { time, data }))
    }

    pub fn recover(&mut self) -> Result<()> {
        unsafe { self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0) }?;
        self.pending.clear();
        self.need_input = 0;
        self.drained = false;
        if let Some(events) = &self.events {
            loop {
                match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                    Ok(_) => {},
                    Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => break,
                    Err(e) => return Err(e.into()),
                }
            }
        }
        unsafe { self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0) }?;
        Ok(())
    }

    pub fn drain(&mut self, out: &mut Vec<Encoded>) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        if self.events.is_some() {
            while !self.pending.is_empty() && Instant::now() < deadline {
                self.poll(out)?;
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        unsafe {
            self.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)?;
            self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)?;
        }
        if self.events.is_some() {
            while !self.drained && Instant::now() < deadline {
                self.poll(out)?;
                std::thread::sleep(Duration::from_millis(2));
            }
        } else {
            self.pull(out)?;
        }
        Ok(())
    }
}

impl Drop for H264 {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            let _ = self.activate.ShutdownObject();
        }
    }
}

fn texture(gpu: &Gpu, size: (u32, u32), staging: bool) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: size.0,
        Height: size.1,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: if staging {
            D3D11_USAGE_STAGING
        } else {
            D3D11_USAGE_DEFAULT
        },
        BindFlags: if staging {
            0
        } else {
            D3D11_BIND_RENDER_TARGET.0 as u32
        },
        CPUAccessFlags: if staging {
            D3D11_CPU_ACCESS_READ.0 as u32
        } else {
            0
        },
        MiscFlags: 0,
    };
    let mut t = None;
    unsafe { gpu.device.CreateTexture2D(&desc, None, Some(&mut t)) }.context("immagine NV12")?;
    t.ok_or_else(|| anyhow!("immagine NV12 mancante"))
}

fn allocator(gpu: &Gpu, size: (u32, u32)) -> Result<IMFVideoSampleAllocatorEx> {
    let media = unsafe { MFCreateMediaType() }?;
    unsafe {
        media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        media.SetUINT64(&MF_MT_FRAME_SIZE, pack(size.0, size.1))?;
    }
    let mut last = None;
    for flags in [
        (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_VIDEO_ENCODER.0) as u32,
        D3D11_BIND_RENDER_TARGET.0 as u32,
    ] {
        let mut raw = std::ptr::null_mut();
        unsafe { MFCreateVideoSampleAllocatorEx(&IMFVideoSampleAllocatorEx::IID, &mut raw) }?;
        let alloc = unsafe { IMFVideoSampleAllocatorEx::from_raw(raw) };
        let mut attrs = None;
        unsafe { MFCreateAttributes(&mut attrs, 2) }?;
        let attrs = attrs.ok_or_else(|| anyhow!("attributi mancanti"))?;
        unsafe {
            attrs.SetUINT32(&MF_SA_D3D11_BINDFLAGS, flags)?;
            attrs.SetUINT32(&MF_SA_D3D11_USAGE, D3D11_USAGE_DEFAULT.0 as u32)?;
            alloc.SetDirectXManager(&gpu.manager)?;
        }
        match unsafe { alloc.InitializeSampleAllocatorEx(4, 12, &attrs, &media) } {
            Ok(()) => return Ok(alloc),
            Err(e) => last = Some(e),
        }
    }
    match last {
        Some(e) => Err(e).context("immagini per l'encoder sulla scheda video"),
        None => bail!("immagini per l'encoder non disponibili"),
    }
}
