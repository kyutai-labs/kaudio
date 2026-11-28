// Copyright (c) Kyutai, all rights reserved.
// This source code is licensed under the license found in the
// LICENSE file in the root directory of this source tree.
//
// Streaming ogg page/packet readers.
// Typical readers wrap a rust io/tokio reader but here we would rather a
// non-blocking api that returns all the pages available at the moment.

use crate::Result;

// https://xiph.org/ogg/doc/framing.html
#[repr(Rust, packed)]
#[derive(Debug, Clone)]
pub struct OggHeader {
    pub capture_pattern: [u8; 4],
    pub version: u8,
    pub header_type: u8,
    pub granule_position: u64,
    pub bitstream_serial: u32,
    pub page_sequence: u32,
    pub checksum: u32,
    pub page_segments: u8,
}

impl OggHeader {
    // Ogg header fields are little-endian, see RFC 3533 section 6.
    // `data` must hold at least `size_of::<OggHeader>()` bytes.
    fn from_le_slice(data: &[u8]) -> Self {
        let u32_at = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        Self {
            capture_pattern: data[0..4].try_into().unwrap(),
            version: data[4],
            header_type: data[5],
            granule_position: u64::from_le_bytes(data[6..14].try_into().unwrap()),
            bitstream_serial: u32_at(14),
            page_sequence: u32_at(18),
            checksum: u32_at(22),
            page_segments: data[26],
        }
    }
}

pub struct Page {
    pub header: OggHeader,
    pub segments: Vec<Vec<u8>>,
}

pub struct PageReader {
    data: Vec<u8>,
}

impl PageReader {
    pub fn new() -> Self {
        Self { data: vec![] }
    }

    pub fn append_bytes(&mut self, data: &[u8]) {
        self.data.extend_from_slice(data)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Page>> {
        let hdr_size = std::mem::size_of::<OggHeader>();
        if self.data.len() < hdr_size {
            return Ok(None);
        }
        let hdr = OggHeader::from_le_slice(&self.data[..hdr_size]);
        if &hdr.capture_pattern != b"OggS" {
            return Err(crate::Error::OggUnexpectedCapturePattern(hdr.capture_pattern));
        }
        if hdr.version != 0 {
            return Err(crate::Error::OggUnsupportedVersion(hdr.version));
        }
        let nsegments = hdr.page_segments as usize;
        if self.data.len() < hdr_size + nsegments {
            return Ok(None);
        }
        let segment_table = &self.data[hdr_size..hdr_size + nsegments];
        let page_size =
            hdr_size + nsegments + segment_table.iter().map(|v| *v as usize).sum::<usize>();
        if self.data.len() < page_size {
            return Ok(None);
        }

        let mut segments = Vec::with_capacity(nsegments);
        let mut start_offset = hdr_size + nsegments;
        for &slen in segment_table.iter() {
            segments.push(self.data[start_offset..start_offset + slen as usize].to_vec());
            start_offset += slen as usize;
        }
        self.data.drain(..page_size);
        Ok(Some(Page { header: hdr, segments }))
    }
}

pub struct PacketReader {
    page_reader: PageReader,
    segments: Vec<Vec<u8>>,
    packets: std::collections::VecDeque<Vec<u8>>,
}

impl PacketReader {
    pub fn new() -> Self {
        Self {
            page_reader: PageReader::new(),
            segments: vec![],
            packets: std::collections::VecDeque::new(),
        }
    }

    pub fn append_bytes(&mut self, data: &[u8]) {
        self.page_reader.append_bytes(data)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Vec<u8>>> {
        while let Some(page) = self.page_reader.next()? {
            for segment in page.segments.into_iter() {
                let slen = segment.len();
                self.segments.push(segment);
                if slen < 255 {
                    let packet = self.segments.concat();
                    self.packets.push_back(packet);
                    self.segments.clear();
                }
            }
        }
        let p = self.packets.pop_front();
        Ok(p)
    }
}

impl Default for PageReader {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for PacketReader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ogg page header fields are little-endian (RFC 3533 section 6), so the decoded values must
    // not depend on the host byte order.
    #[test]
    fn page_header_is_decoded_as_little_endian() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"OggS");
        bytes.extend_from_slice(&[0, 0x02]);
        bytes.extend_from_slice(&[0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
        bytes.extend_from_slice(&[0x44, 0x33, 0x22, 0x11]);
        bytes.extend_from_slice(&[0x88, 0x77, 0x66, 0x55]);
        bytes.extend_from_slice(&[0xdd, 0xcc, 0xbb, 0xaa]);
        bytes.push(1);
        bytes.push(3); // segment table: one 3-byte segment
        bytes.extend_from_slice(&[9, 9, 9]);

        let mut reader = PageReader::new();
        reader.append_bytes(&bytes);
        let page = reader.next().unwrap().unwrap();
        let h = &page.header;
        assert_eq!({ h.granule_position }, 0x0102030405060708);
        assert_eq!({ h.bitstream_serial }, 0x11223344);
        assert_eq!({ h.page_sequence }, 0x55667788);
        assert_eq!({ h.checksum }, 0xaabbccdd);
        assert_eq!(h.page_segments, 1);
        assert_eq!(page.segments, vec![vec![9, 9, 9]]);
    }
}
