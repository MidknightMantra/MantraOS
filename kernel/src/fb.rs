use core::fmt;
use mantra_bootinfo::PixelFormat;

#[derive(Copy, Clone)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

pub struct FrameBuffer {
    pub base: *mut u8,
    pub size: usize,
    pub width: usize,
    pub height: usize,
    pub stride: usize, // pixels per scanline
    pub format: PixelFormat,
}

impl FrameBuffer {
    pub fn put_pixel(&mut self, x: usize, y: usize, c: Rgb) {
        if x >= self.width || y >= self.height {
            return;
        }

        let byte_off = (y * self.stride + x) * 4;
        if byte_off + 4 > self.size {
            return;
        }

        let v = match self.format {
            // UEFI GOP: byte0=R, byte1=G, byte2=B, byte3=reserved
            PixelFormat::Rgb => (c.r as u32) | ((c.g as u32) << 8) | ((c.b as u32) << 16),
            // UEFI GOP: byte0=B, byte1=G, byte2=R, byte3=reserved
            PixelFormat::Bgr => (c.b as u32) | ((c.g as u32) << 8) | ((c.r as u32) << 16),
            PixelFormat::Unknown => (c.r as u32) | ((c.g as u32) << 8) | ((c.b as u32) << 16),
        };

        unsafe {
            core::ptr::write_volatile(self.base.add(byte_off) as *mut u32, v);
        }
    }

    pub fn clear(&mut self, c: Rgb) {
        for y in 0..self.height {
            for x in 0..self.width {
                self.put_pixel(x, y, c);
            }
        }
    }
}

pub struct Console {
    pub fb: FrameBuffer,
    fg: Rgb,
    bg: Rgb,
    cx: usize,
    cy: usize,
    cols: usize,
    rows: usize,
}

impl Console {
    // 8x8 glyph, scaled vertically x2 => 8x16 cell for readability.
    const CELL_W: usize = 8;
    const CELL_H: usize = 16;

    pub fn new(fb: FrameBuffer) -> Self {
        let cols = fb.width / Self::CELL_W;
        let rows = fb.height / Self::CELL_H;
        Self {
            fb,
            fg: Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff,
            },
            bg: Rgb {
                r: 0x00,
                g: 0x00,
                b: 0x00,
            },
            cx: 0,
            cy: 0,
            cols,
            rows,
        }
    }

    pub fn set_colors(&mut self, fg: Rgb, bg: Rgb) {
        self.fg = fg;
        self.bg = bg;
    }

    pub fn clear(&mut self, bg: Rgb) {
        self.bg = bg;
        self.fb.clear(bg);
        self.cx = 0;
        self.cy = 0;
    }

    fn newline(&mut self) {
        self.cx = 0;
        self.cy += 1;
        if self.cy >= self.rows {
            self.cy = self.rows.saturating_sub(1);
        }
    }

    fn glyph(c: u8) -> [u8; 8] {
        // Minimal built-in 8x8 font for diagnostics (subset).
        // Each byte is one row; MSB is leftmost pixel.
        match c {
            b' ' => [0x00; 8],
            b'!' => [0x18, 0x18, 0x18, 0x18, 0x18, 0x00, 0x18, 0x00],
            b'.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00],
            b':' => [0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x00],
            b'/' => [0x06, 0x0c, 0x18, 0x30, 0x60, 0xc0, 0x80, 0x00],
            b'0' => [0x3c, 0x66, 0x6e, 0x76, 0x66, 0x66, 0x3c, 0x00],
            b'1' => [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x3c, 0x00],
            b'2' => [0x3c, 0x66, 0x06, 0x1c, 0x30, 0x66, 0x7e, 0x00],
            b'3' => [0x3c, 0x66, 0x06, 0x1c, 0x06, 0x66, 0x3c, 0x00],
            b'4' => [0x0c, 0x1c, 0x3c, 0x6c, 0x7e, 0x0c, 0x0c, 0x00],
            b'5' => [0x7e, 0x60, 0x7c, 0x06, 0x06, 0x66, 0x3c, 0x00],
            b'6' => [0x1c, 0x30, 0x60, 0x7c, 0x66, 0x66, 0x3c, 0x00],
            b'7' => [0x7e, 0x66, 0x06, 0x0c, 0x18, 0x18, 0x18, 0x00],
            b'8' => [0x3c, 0x66, 0x66, 0x3c, 0x66, 0x66, 0x3c, 0x00],
            b'9' => [0x3c, 0x66, 0x66, 0x3e, 0x06, 0x0c, 0x38, 0x00],
            b'A' => [0x18, 0x3c, 0x66, 0x66, 0x7e, 0x66, 0x66, 0x00],
            b'B' => [0x7c, 0x66, 0x66, 0x7c, 0x66, 0x66, 0x7c, 0x00],
            b'C' => [0x3c, 0x66, 0x60, 0x60, 0x60, 0x66, 0x3c, 0x00],
            b'D' => [0x78, 0x6c, 0x66, 0x66, 0x66, 0x6c, 0x78, 0x00],
            b'E' => [0x7e, 0x60, 0x60, 0x7c, 0x60, 0x60, 0x7e, 0x00],
            b'F' => [0x7e, 0x60, 0x60, 0x7c, 0x60, 0x60, 0x60, 0x00],
            b'G' => [0x3c, 0x66, 0x60, 0x6e, 0x66, 0x66, 0x3c, 0x00],
            b'I' => [0x3c, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3c, 0x00],
            b'K' => [0x66, 0x6c, 0x78, 0x70, 0x78, 0x6c, 0x66, 0x00],
            b'L' => [0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x7e, 0x00],
            b'M' => [0x63, 0x77, 0x7f, 0x6b, 0x63, 0x63, 0x63, 0x00],
            b'N' => [0x66, 0x76, 0x7e, 0x7e, 0x6e, 0x66, 0x66, 0x00],
            b'O' => [0x3c, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3c, 0x00],
            b'R' => [0x7c, 0x66, 0x66, 0x7c, 0x78, 0x6c, 0x66, 0x00],
            b'S' => [0x3c, 0x66, 0x30, 0x18, 0x0c, 0x66, 0x3c, 0x00],
            b'T' => [0x7e, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00],
            b'V' => [0x66, 0x66, 0x66, 0x66, 0x66, 0x3c, 0x18, 0x00],
            b'X' => [0x66, 0x66, 0x3c, 0x18, 0x3c, 0x66, 0x66, 0x00],
            b'Y' => [0x66, 0x66, 0x3c, 0x18, 0x18, 0x18, 0x18, 0x00],
            b'_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7e, 0x00],
            b'a'..=b'z' => Self::glyph(c - 32), // cheap lowercase->uppercase for now
            _ => [0x7e, 0x42, 0x5a, 0x5a, 0x5a, 0x42, 0x7e, 0x00], // "unknown"
        }
    }

    fn put_char(&mut self, ch: u8) {
        if ch == b'\n' {
            self.newline();
            return;
        }
        if ch == b'\r' {
            self.cx = 0;
            return;
        }

        if self.cx >= self.cols {
            self.newline();
        }

        let glyph = Self::glyph(ch);
        let px0 = self.cx * Self::CELL_W;
        let py0 = self.cy * Self::CELL_H;

        for (row, bits) in glyph.iter().copied().enumerate() {
            for col in 0..8 {
                let on = (bits & (0x80 >> col)) != 0;
                let color = if on { self.fg } else { self.bg };
                let y = py0 + row * 2;
                self.fb.put_pixel(px0 + col, y, color);
                self.fb.put_pixel(px0 + col, y + 1, color);
            }
        }

        self.cx += 1;
    }
}

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            self.put_char(b);
        }
        Ok(())
    }
}
