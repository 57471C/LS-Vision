use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub channels: u32, // 3 = RGB
    pub data: Vec<u8>, // row-major RGB, width * height * 3
    pub timestamp_ms: u64,
}

impl RawFrame {
    /// Crop a rectangle. Returns RGB bytes of the crop.
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((w * h * self.channels) as usize);
        let row_stride = (self.width * self.channels) as usize;
        let crop_stride = (w * self.channels) as usize;
        for row in y..(y + h) {
            let start = (row as usize * row_stride) + (x as usize * self.channels as usize);
            let end = start + crop_stride;
            out.extend_from_slice(&self.data[start..end]);
        }
        out
    }
}

pub struct FrameState {
    pub latest: Arc<Mutex<Option<RawFrame>>>,
}
