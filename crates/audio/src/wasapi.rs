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
    next_timestamp_us: u64,
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

            audio_client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK,
                    10_000_000,
                    0,
                    mix_fmt_ptr,
                    None,
                )
                .context("IAudioClient::Initialize(loopback)")?;

            let capture_client: IAudioCaptureClient = audio_client
                .GetService()
                .context("IAudioClient::GetService(IAudioCaptureClient)")?;
            audio_client.Start().context("IAudioClient::Start")?;

            CoTaskMemFree(Some(mix_fmt_ptr as _));

            Ok(Self {
                audio_client,
                capture_client,
                sample_rate: mix_fmt.nSamplesPerSec,
                channels: mix_fmt.nChannels,
                bits_per_sample: mix_fmt.wBitsPerSample,
                next_timestamp_us: 0,
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
        loop {
            if packet_frames == 0 {
                break;
            }
            let mut data_ptr = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            unsafe {
                self.capture_client
                    .GetBuffer(&mut data_ptr, &mut frames, &mut flags, None, None)
                    .context("IAudioCaptureClient::GetBuffer")?;
            }

            if frames > 0 {
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
        let frames_read = (all.len() / self.channels as usize) as u64;
        let chunk = PcmChunk {
            timestamp_us: self.next_timestamp_us,
            sample_rate: self.sample_rate,
            channels: self.channels,
            samples_f32: all,
        };
        self.next_timestamp_us += frames_read * 1_000_000 / u64::from(self.sample_rate);
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

