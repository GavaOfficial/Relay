use std::mem::ManuallyDrop;

use anyhow::{anyhow, Context, Result};
use windows::Win32::{Media::MediaFoundation::*, System::Com::CoTaskMemFree};

use crate::{aac::SAMPLES_PER_FRAME, clock::AUDIO_RATE, mix::CHANNELS};

pub struct AacEncoder {
    mft: IMFTransform,
    output_size: u32,
    provides_samples: bool,
    next_out: Option<u64>,
}

pub struct AacFrame {
    pub sample: u64,
    pub data: Vec<u8>,
}

fn output_type(bytes_per_sec: u32) -> Result<IMFMediaType> {
    let t = unsafe { MFCreateMediaType() }?;
    unsafe {
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
        t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS as u32)?;
        t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bytes_per_sec)?;
        t.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        t.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)?;
    }
    Ok(t)
}

fn input_type() -> Result<IMFMediaType> {
    let t = unsafe { MFCreateMediaType() }?;
    let block = 2 * CHANNELS as u32;
    unsafe {
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
        t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS as u32)?;
        t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block)?;
        t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, block * AUDIO_RATE)?;
    }
    Ok(t)
}

impl AacEncoder {
    pub fn open() -> Result<Self> {
        let input = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Audio,
            guidSubtype: MFAudioFormat_PCM,
        };
        let output = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Audio,
            guidSubtype: MFAudioFormat_AAC,
        };
        let mut ptr: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        unsafe {
            MFTEnumEx(
                MFT_CATEGORY_AUDIO_ENCODER,
                MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
                Some(&input),
                Some(&output),
                &mut ptr,
                &mut count,
            )
        }
        .context("ricerca dell'encoder AAC")?;
        let mut activates = Vec::new();
        if !ptr.is_null() {
            unsafe {
                for a in std::slice::from_raw_parts_mut(ptr, count as usize) {
                    if let Some(a) = a.take() {
                        activates.push(a);
                    }
                }
                CoTaskMemFree(Some(ptr as *const _));
            }
        }
        let activate = activates
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("encoder AAC di Windows non trovato"))?;
        let mft: IMFTransform =
            unsafe { activate.ActivateObject() }.context("avvio dell'encoder AAC")?;

        let mut configured = false;
        for rate in [20_000u32, 16_000, 24_000, 12_000] {
            let out = output_type(rate)?;
            let inp = input_type()?;
            let first = unsafe { mft.SetOutputType(0, &out, 0) }
                .and_then(|_| unsafe { mft.SetInputType(0, &inp, 0) });
            let second = || {
                unsafe { mft.SetInputType(0, &inp, 0) }
                    .and_then(|_| unsafe { mft.SetOutputType(0, &out, 0) })
            };
            if first.is_ok() || second().is_ok() {
                configured = true;
                break;
            }
        }
        if !configured {
            return Err(anyhow!("l'encoder AAC non accetta audio a 48 kHz stereo"));
        }
        let info = unsafe { mft.GetOutputStreamInfo(0) }?;
        unsafe {
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        }
        Ok(Self {
            mft,
            output_size: info.cbSize.max(8192),
            provides_samples: info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0)
                    as u32
                != 0,
            next_out: None,
        })
    }

    pub fn encode(&mut self, pcm: &[u8], first_sample: u64, out: &mut Vec<AacFrame>) -> Result<()> {
        if pcm.is_empty() {
            return Ok(());
        }
        if self.next_out.is_none() {
            self.next_out = Some(first_sample);
        }
        let frames = (pcm.len() / (2 * CHANNELS)) as u64;
        let sample = unsafe { MFCreateSample() }?;
        let buffer = unsafe { MFCreateMemoryBuffer(pcm.len() as u32) }?;
        unsafe {
            let mut dst = std::ptr::null_mut();
            buffer.Lock(&mut dst, None, None)?;
            std::ptr::copy_nonoverlapping(pcm.as_ptr(), dst, pcm.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(pcm.len() as u32)?;
            sample.AddBuffer(&buffer)?;
            sample
                .SetSampleTime((first_sample as i128 * 10_000_000 / AUDIO_RATE as i128) as i64)?;
            sample.SetSampleDuration((frames as i128 * 10_000_000 / AUDIO_RATE as i128) as i64)?;
        }
        let mut tries = 0;
        loop {
            match unsafe { self.mft.ProcessInput(0, &sample, 0) } {
                Ok(()) => break,
                Err(e) if e.code() == MF_E_NOTACCEPTING && tries < 4 => {
                    tries += 1;
                    self.pull(out)?;
                }
                Err(e) => return Err(e).context("invio dell'audio all'encoder AAC"),
            }
        }
        self.pull(out)
    }

    fn pull(&mut self, out: &mut Vec<AacFrame>) -> Result<()> {
        loop {
            let sample = if self.provides_samples {
                None
            } else {
                let s = unsafe { MFCreateSample() }?;
                let b = unsafe { MFCreateMemoryBuffer(self.output_size) }?;
                unsafe { s.AddBuffer(&b) }?;
                Some(s)
            };
            let mut buffers = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let result = unsafe { self.mft.ProcessOutput(0, &mut buffers, &mut status) };
            let sample = unsafe { ManuallyDrop::take(&mut buffers[0].pSample) };
            drop(unsafe { ManuallyDrop::take(&mut buffers[0].pEvents) });
            match result {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(e) => return Err(e).context("lettura dall'encoder AAC"),
            }
            let Some(sample) = sample else {
                return Ok(());
            };
            let buffer = unsafe { sample.ConvertToContiguousBuffer() }?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }?;
            let data = unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec();
            unsafe { buffer.Unlock() }?;
            if data.is_empty() {
                continue;
            }
            let at = self.next_out.unwrap_or(0);
            self.next_out = Some(at + SAMPLES_PER_FRAME);
            out.push(AacFrame { sample: at, data });
        }
    }
}
