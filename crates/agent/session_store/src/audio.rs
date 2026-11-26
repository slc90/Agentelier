use agentelier_conversation::{
    AudioAttachment, AudioFormat, AudioId, MAX_AUDIO_BYTES, MAX_AUDIO_NAME_CHARS,
};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Cursor, Read};
use std::path::Path;
use std::sync::Arc;
use symphonia::core::codecs::audio::well_known::{CODEC_ID_FLAC, CODEC_ID_MP3};
use symphonia::core::formats::{TrackType, probe::Hint};
use symphonia::core::io::MediaSourceStream;

pub(super) struct ImportedAudio {
    pub(super) attachment: AudioAttachment,
    pub(super) bytes: Arc<[u8]>,
}

pub(super) fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
    Draft,
    Published,
}

pub(super) fn import(path: &Path, source: Source) -> io::Result<ImportedAudio> {
    let name: String = path
        .file_name()
        .ok_or_else(|| io::Error::other("请选择音频文件"))?
        .to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_control() {
                '�'
            } else {
                character
            }
        })
        .take(MAX_AUDIO_NAME_CHARS)
        .collect();
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("所选路径不是普通音频文件"));
    }
    if metadata.len() > MAX_AUDIO_BYTES as u64 {
        return Err(io::Error::other("所选音频文件超过 20 MiB"));
    }
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_AUDIO_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_AUDIO_BYTES {
        return Err(io::Error::other("所选音频文件超过 20 MiB"));
    }
    let final_metadata = file.metadata()?;
    if bytes.len() as u64 != metadata.len()
        || bytes.is_empty()
        || final_metadata.len() != metadata.len()
        || final_metadata.modified()? != metadata.modified()?
    {
        return Err(io::Error::other("音频为空或文件在读取期间变化"));
    }
    let bytes: Arc<[u8]> = bytes.into();
    let (format, duration_ms) = inspect(bytes.clone(), source)?;
    let attachment = AudioAttachment {
        id: AudioId::new_v4(),
        name,
        format,
        digest: checksum(&bytes),
        encoded_bytes: bytes.len(),
        duration_ms,
    };
    attachment.validate().map_err(io::Error::other)?;
    Ok(ImportedAudio { attachment, bytes })
}

fn inspect(bytes: Arc<[u8]>, source: Source) -> io::Result<(AudioFormat, u64)> {
    let wav = bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE");
    if wav
        && let Some(header) = bytes
            .get(4..8)
            .and_then(|slice| <[u8; 4]>::try_from(slice).ok())
        && u64::from(u32::from_le_bytes(header)) + 8 > bytes.len() as u64
    {
        return Err(io::Error::other("WAV 文件内容不完整"));
    }
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut reader = symphonia::default::get_probe()
        .probe(&Hint::new(), stream, Default::default(), Default::default())
        .map_err(|failure| invalid_audio(&failure))?;
    let track = reader
        .default_track(TrackType::Audio)
        .ok_or_else(|| io::Error::other("文件中没有音频轨道"))?;
    let parameters = track
        .codec_params
        .as_ref()
        .and_then(|parameter| parameter.audio())
        .ok_or_else(|| io::Error::other("音频轨道缺少解码参数"))?;
    let format = if wav {
        AudioFormat::Wav
    } else if parameters.codec == CODEC_ID_MP3 {
        AudioFormat::Mp3
    } else if source == Source::Published && parameters.codec == CODEC_ID_FLAC {
        AudioFormat::Flac
    } else {
        return Err(io::Error::other(match source {
            Source::Draft => "支持 PCM WAV 或 MP3 音频",
            Source::Published => "支持 MP3、FLAC 或 WAV 音频",
        }));
    };
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(parameters, &Default::default())
        .map_err(|failure| invalid_audio(&failure))?;
    let mut rate = None;
    let mut frames = 0_u64;
    while let Some(packet) = reader
        .next_packet()
        .map_err(|failure| invalid_audio(&failure))?
    {
        if packet.track_id != id {
            continue;
        }
        let audio = decoder
            .decode(&packet)
            .map_err(|failure| invalid_audio(&failure))?;
        let packet_rate = u64::from(audio.spec().rate());
        if packet_rate == 0 || *rate.get_or_insert(packet_rate) != packet_rate {
            return Err(io::Error::other("文件包含无效或变化的音频采样率"));
        }
        frames += audio.frames() as u64;
    }
    let rate = rate
        .filter(|_| frames != 0)
        .ok_or_else(|| io::Error::other("音频没有可解码的样本"))?;
    Ok((format, (frames * 1000).div_ceil(rate)))
}

fn invalid_audio(failure: &symphonia::core::errors::Error) -> io::Error {
    io::Error::other(format!("音频格式不受支持或内容损坏：{failure}"))
}
