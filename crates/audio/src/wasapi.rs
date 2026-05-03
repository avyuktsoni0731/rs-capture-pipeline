use std::time::Instant;

use anyhow::Context;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, MMDeviceEnumerator,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};

use crate::{AudioCapture, PcmChunk};

pub struct WasapiLoopbackCapture {
    audio_client: IAudioClient,
    capture_client: IAudioCaptureClient,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    /// Wall time right before `IAudioClient::Start` (for diagnostics / future sync).
    stream_started_at: Instant,
}

impl WasapiLoopbackCapture {
    pub fn new() -> anyhow::Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .context("CoCreateInstance MMDeviceEnumerator")?;
            let device: IMMDevice = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .context("GetDefaultAudioEndpoint(eRender,eConsole)")?;
            let audio_client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .context("IMMDevice::Activate(IAudioClient)")?;

            let mix_fmt_ptr = audio_client.GetMixFormat().context("GetMixFormat")?;
            let mix_fmt: WAVEFORMATEX = *mix_fmt_ptr;

            // Keep initial implementation simple: expect 32-bit float mix format.
            let is_ieee_float = mix_fmt.wBitsPerSample == 32;
            let w_format_tag = mix_fmt.wFormatTag;
            anyhow::ensure!(
                is_ieee_float,
                "WASAPI mix format must be IEEE_FLOAT for now (wFormatTag={w_format_tag})"
            );

            // 100-ns units. ~1s (`10_000_000`) adds huge latency; `0` can be too small when the video
            // thread is blocked on encode (underruns → crackle). ~50ms is a common stable shared-mode size.
            const SHARED_LOOPBACK_BUFFER_100NS: i64 = 500_000;
            audio_client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK,
                    SHARED_LOOPBACK_BUFFER_100NS,
                    0,
                    mix_fmt_ptr,
                    None,
                )
                .context("IAudioClient::Initialize(loopback)")?;

            let capture_client: IAudioCaptureClient = audio_client
                .GetService()
                .context("IAudioClient::GetService(IAudioCaptureClient)")?;
            let stream_started_at = Instant::now();
            audio_client.Start().context("IAudioClient::Start")?;

            CoTaskMemFree(Some(mix_fmt_ptr as _));

            Ok(Self {
                audio_client,
                capture_client,
                sample_rate: mix_fmt.nSamplesPerSec,
                channels: mix_fmt.nChannels,
                bits_per_sample: mix_fmt.wBitsPerSample,
                stream_started_at,
            })
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn bits_per_sample(&self) -> u16 {
        self.bits_per_sample
    }

    /// When the capture stream entered the running state (right before `Start`).
    pub fn stream_started_at(&self) -> Instant {
        self.stream_started_at
    }
}

impl AudioCapture for WasapiLoopbackCapture {
    fn try_read_chunk(&mut self) -> anyhow::Result<Option<PcmChunk>> {
        let mut packet_frames = unsafe {
            self.capture_client
                .GetNextPacketSize()
                .context("IAudioCaptureClient::GetNextPacketSize")?
        };
        if packet_frames == 0 {
            return Ok(None);
        }

        let mut all = Vec::<f32>::new();
        let mut chunk_start_frame: Option<u64> = None;
        loop {
            if packet_frames == 0 {
                break;
            }
            let mut data_ptr = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            let mut device_position = 0u64;
            unsafe {
                self.capture_client
                    .GetBuffer(
                        &mut data_ptr,
                        &mut frames,
                        &mut flags,
                        Some(&mut device_position),
                        None,
                    )
                    .context("IAudioCaptureClient::GetBuffer")?;
            }

            if frames > 0 {
                if chunk_start_frame.is_none() {
                    chunk_start_frame = Some(device_position);
                }
                let samples = frames as usize * self.channels as usize;
                if (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 {
                    all.resize(all.len() + samples, 0.0);
                } else {
                    let slice = unsafe { std::slice::from_raw_parts(data_ptr as *const f32, samples) };
                    all.extend_from_slice(slice);
                }
            }

            unsafe {
                self.capture_client
                    .ReleaseBuffer(frames)
                    .context("IAudioCaptureClient::ReleaseBuffer")?;
                packet_frames = self
                    .capture_client
                    .GetNextPacketSize()
                    .context("IAudioCaptureClient::GetNextPacketSize")?;
            }
        }

        if all.is_empty() {
            return Ok(None);
        }
        let start_frame = chunk_start_frame.unwrap_or(0);
        let timestamp_us = start_frame
            .saturating_mul(1_000_000)
            .saturating_div(u64::from(self.sample_rate));
        let chunk = PcmChunk {
            timestamp_us,
            sample_rate: self.sample_rate,
            channels: self.channels,
            samples_f32: all,
        };
        Ok(Some(chunk))
    }
}

impl Drop for WasapiLoopbackCapture {
    fn drop(&mut self) {
        unsafe {
            let _ = self.audio_client.Stop();
        }
    }
}

