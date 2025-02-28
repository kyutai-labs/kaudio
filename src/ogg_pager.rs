// Copyright (c) Kyutai, all rights reserved.
// This source code is licensed under the license found in the
// LICENSE file in the root directory of this source tree.
//
// Streaming ogg page/packet readers.
// Typical readers wrap a rust io/tokio reader but here we would rather a
// non-blocking api that returns all the pages available at the moment.

use anyhow::Result;

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
        let hdr: OggHeader =
            unsafe { std::ptr::read_unaligned(self.data.as_ptr() as *const OggHeader) };
        if &hdr.capture_pattern != b"OggS" {
            anyhow::bail!("unexpected capture pattern {:#?}", hdr.capture_pattern)
        }
        if hdr.version != 0 {
            anyhow::bail!("unsupported ogg version {}", hdr.version)
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
