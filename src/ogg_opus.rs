// Copyright (c) Kyutai, all rights reserved.
// This source code is licensed under the license found in the
// LICENSE file in the root directory of this source tree.

use crate::Result;

// `repr(C)` fixes the fields in their on-wire order and `packed` removes padding, so the struct
// has the layout of the 19-byte ID header and can be read straight from the bytes.
#[repr(C, packed)]
#[derive(Debug, Clone)]
pub struct OpusHead {
    pub magic_signature: [u8; 8],
    pub version: u8,
    pub channel_count: u8,
    pub pre_skip: u16,
    pub sample_rate: u32,
    pub output_gain: i16,
    pub mapping_family: u8,
}

const _: () = assert!(std::mem::size_of::<OpusHead>() == 19);

impl OpusHead {
    pub fn from_slice(data: &[u8]) -> Result<Self> {
        let l = std::mem::size_of::<OpusHead>();
        if data.len() != l {
            return Err(crate::Error::OggUnexpectedLenForOpusHead(data.len()));
        }
        let head = Self::from_le_slice(data);
        if &head.magic_signature != b"OpusHead" {
            return Err(crate::Error::OggUnexpectedSignature(head.magic_signature));
        }
        Ok(head)
    }

    /// Decodes the ID header at the start of `data`. Its fields are little-endian (RFC 7845
    /// section 5.1), the host's byte order here, so the bytes are copied as they are.
    #[cfg(target_endian = "little")]
    fn from_le_slice(data: &[u8]) -> Self {
        assert!(data.len() >= std::mem::size_of::<Self>());
        // SAFETY: `data` holds at least `size_of::<Self>()` bytes, `read_unaligned` has no
        // alignment requirement, and every bit pattern is a valid value for the integer fields.
        unsafe { std::ptr::read_unaligned(data.as_ptr().cast::<Self>()) }
    }

    /// Decodes the ID header at the start of `data`. Its fields are little-endian (RFC 7845
    /// section 5.1), so each one is converted to the host's byte order.
    #[cfg(target_endian = "big")]
    fn from_le_slice(data: &[u8]) -> Self {
        Self::parse_le(data)
    }

    /// Decodes the ID header at the start of `data` field by field, whatever the host's byte
    /// order. `data` must hold at least `size_of::<OpusHead>()` bytes.
    #[cfg(any(target_endian = "big", test))]
    fn parse_le(data: &[u8]) -> Self {
        Self {
            magic_signature: data[0..8].try_into().unwrap(),
            version: data[8],
            channel_count: data[9],
            pre_skip: u16::from_le_bytes(data[10..12].try_into().unwrap()),
            sample_rate: u32::from_le_bytes(data[12..16].try_into().unwrap()),
            output_gain: i16::from_le_bytes(data[16..18].try_into().unwrap()),
            mapping_family: data[18],
        }
    }
}

// This must be an allowed value among 120, 240, 480, 960, 1920, and 2880.
// Using a different value would result in a BadArg "invalid argument" error when calling encode.
// https://opus-codec.org/docs/opus_api-1.2/group__opus__encoder.html#ga4ae9905859cd241ef4bb5c59cd5e5309
const OPUS_ENCODER_FRAME_SIZE: usize = 960;

pub struct Encoder {
    pw: ogg::PacketWriter<'static, Vec<u8>>,
    encoder: opus2::Encoder,
    total_data: usize,
    sample_rate: usize,
    header_data: Vec<u8>,
    out_pcm: std::collections::VecDeque<f32>,
    opus_buf: Vec<u8>,
}

fn write_opus_header<W: std::io::Write>(w: &mut W) -> std::io::Result<()> {
    use byteorder::WriteBytesExt;

    // https://wiki.xiph.org/OggOpus#ID_Header
    w.write_all(b"OpusHead")?;
    w.write_u8(1)?; // version
    w.write_u8(1)?; // channel count
    w.write_u16::<byteorder::LittleEndian>(3840)?; // pre-skip
    w.write_u32::<byteorder::LittleEndian>(48000)?; //  sample-rate in Hz
    w.write_i16::<byteorder::LittleEndian>(0)?; // output gain Q7.8 in dB
    w.write_u8(0)?; // channel map
    Ok(())
}

fn write_opus_tags<W: std::io::Write>(w: &mut W, tags: &[(&str, &str)]) -> std::io::Result<()> {
    use byteorder::WriteBytesExt;

    // https://wiki.xiph.org/OggOpus#Comment_Header
    let vendor = "KyutaiMoshi";
    w.write_all(b"OpusTags")?;
    w.write_u32::<byteorder::LittleEndian>(vendor.len() as u32)?; // vendor string length
    w.write_all(vendor.as_bytes())?; // vendor string, UTF8 encoded
    w.write_u32::<byteorder::LittleEndian>(tags.len() as u32)?; // number of tags
    for (name, value) in tags {
        // Each tag is a `NAME=value` user comment, see
        // https://www.xiph.org/vorbis/doc/v-comment.html
        let comment = format!("{name}={value}");
        w.write_u32::<byteorder::LittleEndian>(comment.len() as u32)?;
        w.write_all(comment.as_bytes())?;
    }
    Ok(())
}

impl Encoder {
    pub fn new(sample_rate: usize) -> Result<Self> {
        Self::new_with_tags(sample_rate, &[])
    }

    /// An encoder whose comment header carries `tags`, `(name, value)` pairs
    /// written as `NAME=value` user comments (e.g. `("TITLE", "...")`). The
    /// names should be printable ASCII without `=`.
    pub fn new_with_tags(sample_rate: usize, tags: &[(&str, &str)]) -> Result<Self> {
        let encoder = opus2::Encoder::new(
            sample_rate as u32,
            opus2::Channels::Mono,
            opus2::Application::Voip,
        )?;
        let all_data = Vec::new();
        let mut pw = ogg::PacketWriter::new(all_data);
        let mut head = Vec::new();
        write_opus_header(&mut head)?;
        pw.write_packet(head, 42, ogg::PacketWriteEndInfo::EndPage, 0)?;
        let mut tags_packet = Vec::new();
        write_opus_tags(&mut tags_packet, tags)?;
        pw.write_packet(tags_packet, 42, ogg::PacketWriteEndInfo::EndPage, 0)?;
        let header_data = {
            let inner = pw.inner_mut();
            let data = inner.to_vec();
            inner.clear();
            data
        };
        let out_pcm = std::collections::VecDeque::with_capacity(2 * OPUS_ENCODER_FRAME_SIZE);
        let opus_buf = vec![0u8; 50_000];
        Ok(Self { encoder, pw, header_data, total_data: 0, out_pcm, opus_buf, sample_rate })
    }

    pub fn header_data(&self) -> &[u8] {
        self.header_data.as_slice()
    }

    pub fn encode_page(&mut self, pcm: &[f32]) -> Result<Vec<u8>> {
        let mut encoded = vec![];
        self.out_pcm.extend(pcm.iter());
        let nchunks = self.out_pcm.len() / OPUS_ENCODER_FRAME_SIZE;
        for _chunk_id in 0..nchunks {
            let mut chunk = Vec::with_capacity(OPUS_ENCODER_FRAME_SIZE);
            for _i in 0..OPUS_ENCODER_FRAME_SIZE {
                let v = match self.out_pcm.pop_front() {
                    None => return Err(crate::Error::OpusMissingPcm),
                    Some(v) => v,
                };
                chunk.push(v)
            }
            self.total_data += chunk.len();
            let size = self.encoder.encode_float(&chunk, &mut self.opus_buf)?;
            // The granule position uses a fixed rate of 48kHz even if the underlying audio uses a
            // different rate.
            // This does not matter when reading ogg files in chrome but should be set properly for
            // VLC to work.
            let absgp = self.total_data as u64 * 48_000 / self.sample_rate as u64;
            if size > 0 {
                self.pw.write_packet(
                    self.opus_buf[..size].to_vec(),
                    42,
                    ogg::PacketWriteEndInfo::EndPage,
                    absgp,
                )?;
                let data = self.pw.inner_mut();
                if !data.is_empty() {
                    encoded.extend_from_slice(data);
                    data.clear()
                }
            }
        }
        Ok(encoded)
    }
}

pub struct AsyncDecoder {
    pr_ogg: ogg::reading::async_api::PacketReader<tokio::io::DuplexStream>,
    decoder: opus2::Decoder,
    pcm_buf: Vec<f32>,
    size_in_buf: usize,
    flush_every_n_samples: usize,
}

pub type Sender = tokio::sync::mpsc::UnboundedSender<Vec<u8>>;

impl AsyncDecoder {
    pub fn new(sample_rate: usize, flush_every_n_samples: usize) -> Result<(Self, Sender)> {
        use tokio::io::AsyncWriteExt;

        let pcm_buf = vec![0f32; flush_every_n_samples + sample_rate * 5];
        let (mut tx_tokio, rx_tokio) = tokio::io::duplex(100_000);
        let (tx_sync, mut rx_sync) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let pr_ogg = ogg::reading::async_api::PacketReader::new(rx_tokio);
        let decoder = opus2::Decoder::new(sample_rate as u32, opus2::Channels::Mono)?;
        tokio::task::spawn(async move {
            // It is important to use a tokio mpsc channel here to avoid starving the other
            // threads.
            while let Some(data) = rx_sync.recv().await {
                tx_tokio.write_all(&data).await?;
            }
            Ok::<_, crate::Error>(())
        });
        let s = Self { pr_ogg, decoder, pcm_buf, size_in_buf: 0, flush_every_n_samples };
        Ok((s, tx_sync))
    }

    pub async fn read(&mut self) -> Result<Option<&[f32]>> {
        use futures_util::StreamExt;

        loop {
            let packet = match self.pr_ogg.next().await {
                None => return Ok(None),
                Some(v) => v?,
            };
            if packet.data.starts_with(b"OpusHead") || packet.data.starts_with(b"OpusTags") {
                continue;
            }
            let read_size = self.decoder.decode_float(
                &packet.data,
                &mut self.pcm_buf[self.size_in_buf..],
                /* Forward Error Correction */ false,
            )?;
            self.size_in_buf += read_size;
            // flush the data every half timestep
            if self.size_in_buf >= self.flush_every_n_samples {
                let size_in_buf = self.size_in_buf;
                self.size_in_buf = 0;
                return Ok(Some(&self.pcm_buf[..size_in_buf]));
            }
        }
    }
}

pub struct Decoder {
    pr_ogg: crate::ogg_pager::PacketReader,
    decoder: opus2::Decoder,
    pcm_buf: Vec<f32>,
    size_in_buf: usize,
    flush_every_n_samples: usize,
}

impl Decoder {
    pub fn new(sample_rate: usize, flush_every_n_samples: usize) -> Result<Self> {
        let pcm_buf = vec![0f32; flush_every_n_samples + sample_rate * 5];
        let pr_ogg = crate::ogg_pager::PacketReader::new();
        let decoder = opus2::Decoder::new(sample_rate as u32, opus2::Channels::Mono)?;
        let s = Self { pr_ogg, decoder, pcm_buf, size_in_buf: 0, flush_every_n_samples };
        Ok(s)
    }

    pub fn decode(&mut self, data: &[u8]) -> Result<Option<&[f32]>> {
        self.pr_ogg.append_bytes(data);
        while let Some(packet) = self.pr_ogg.next()? {
            if packet.starts_with(b"OpusHead") || packet.starts_with(b"OpusTags") {
                continue;
            }
            let read_size = self.decoder.decode_float(
                &packet,
                &mut self.pcm_buf[self.size_in_buf..],
                /* Forward Error Correction */ false,
            )?;
            self.size_in_buf += read_size;
        }
        let pcm = if self.size_in_buf >= self.flush_every_n_samples {
            let size_in_buf = self.size_in_buf;
            self.size_in_buf = 0;
            Some(&self.pcm_buf[..size_in_buf])
        } else {
            None
        };
        Ok(pcm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // OpusHead fields are little-endian (RFC 7845 section 5.1), independent of the host. Both
    // the decoding used on this host and the field-by-field one used on big-endian hosts are
    // checked.
    // https://www.rfc-editor.org/info/rfc7845/#section-5.1
    #[test]
    fn opus_head_is_decoded_as_little_endian() -> Result<()> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"OpusHead");
        bytes.extend_from_slice(&[1, 2]);
        bytes.extend_from_slice(&[0x02, 0x01]); // pre_skip
        bytes.extend_from_slice(&[0x44, 0x33, 0x22, 0x11]); // sample_rate
        bytes.extend_from_slice(&[0xfe, 0xff]); // output_gain = -2
        bytes.push(0);
        for head in [OpusHead::from_slice(&bytes)?, OpusHead::parse_le(&bytes)] {
            assert_eq!(&head.magic_signature, b"OpusHead");
            assert_eq!({ head.pre_skip }, 0x0102);
            assert_eq!({ head.sample_rate }, 0x11223344);
            assert_eq!({ head.output_gain }, -2);
            assert_eq!(head.channel_count, 2);
        }
        Ok(())
    }

    #[test]
    fn opus_tags() -> Result<()> {
        let tags = [("TITLE", "Hello"), ("ARTIST", "Kyutai")];
        let encoder = Encoder::new_with_tags(24000, &tags)?;
        let mut reader = ogg::PacketReader::new(std::io::Cursor::new(encoder.header_data()));
        let head = reader.read_packet_expected()?;
        assert!(head.data.starts_with(b"OpusHead"));
        let packet = reader.read_packet_expected()?.data;
        let mut expected = b"OpusTags".to_vec();
        expected.extend_from_slice(&11u32.to_le_bytes());
        expected.extend_from_slice(b"KyutaiMoshi");
        expected.extend_from_slice(&2u32.to_le_bytes());
        expected.extend_from_slice(&11u32.to_le_bytes());
        expected.extend_from_slice(b"TITLE=Hello");
        expected.extend_from_slice(&13u32.to_le_bytes());
        expected.extend_from_slice(b"ARTIST=Kyutai");
        assert_eq!(packet, expected);
        // Without tags, the comment header is left empty.
        let encoder = Encoder::new(24000)?;
        let mut reader = ogg::PacketReader::new(std::io::Cursor::new(encoder.header_data()));
        reader.read_packet_expected()?;
        assert_eq!(reader.read_packet_expected()?.data.len(), 8 + 4 + 11 + 4);
        Ok(())
    }
}
