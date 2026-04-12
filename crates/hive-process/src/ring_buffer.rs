/// A fixed-capacity circular byte buffer.
///
/// When the buffer is full, new writes overwrite the oldest data.
pub struct RingBuffer {
    buf: Vec<u8>,
    capacity: usize,
    write_pos: usize,
    len: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "RingBuffer capacity must be > 0");
        Self { buf: vec![0u8; capacity], capacity, write_pos: 0, len: 0 }
    }

    /// Append bytes, overwriting the oldest data when the buffer is full.
    pub fn write(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        if data.len() >= self.capacity {
            // Data larger than buffer: keep only the tail.
            let start = data.len() - self.capacity;
            self.buf.copy_from_slice(&data[start..]);
            self.write_pos = 0;
            self.len = self.capacity;
            return;
        }

        let first_chunk = (self.capacity - self.write_pos).min(data.len());
        self.buf[self.write_pos..self.write_pos + first_chunk]
            .copy_from_slice(&data[..first_chunk]);

        if first_chunk < data.len() {
            let remaining = data.len() - first_chunk;
            self.buf[..remaining].copy_from_slice(&data[first_chunk..]);
        }

        self.write_pos = (self.write_pos + data.len()) % self.capacity;
        self.len = (self.len + data.len()).min(self.capacity);
    }

    /// Read all buffered data in chronological order.
    pub fn read_all(&self) -> Vec<u8> {
        if self.len == 0 {
            return Vec::new();
        }
        let start = if self.len < self.capacity { 0 } else { self.write_pos };
        let mut result = Vec::with_capacity(self.len);
        for i in 0..self.len {
            result.push(self.buf[(start + i) % self.capacity]);
        }
        result
    }

    /// Read all buffered data as a lossy UTF-8 string.
    pub fn read_all_string(&self) -> String {
        String::from_utf8_lossy(&self.read_all()).into_owned()
    }

    /// Read the last `n` lines from the buffer.
    /// Returns an empty string when `n == 0`.
    pub fn read_tail_lines(&self, n: usize) -> String {
        if n == 0 {
            return String::new();
        }
        let all = self.read_all_string();
        let lines: Vec<&str> = all.lines().collect();
        let start = lines.len().saturating_sub(n);
        lines[start..].join("\n")
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer() {
        let buf = RingBuffer::new(16);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.read_all(), Vec::<u8>::new());
        assert_eq!(buf.read_all_string(), "");
    }

    #[test]
    fn write_within_capacity() {
        let mut buf = RingBuffer::new(16);
        buf.write(b"hello");
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.read_all_string(), "hello");
    }

    #[test]
    fn write_exactly_capacity() {
        let mut buf = RingBuffer::new(5);
        buf.write(b"abcde");
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.read_all_string(), "abcde");
    }

    #[test]
    fn write_overflow_preserves_order() {
        let mut buf = RingBuffer::new(8);
        buf.write(b"abcdef"); // 6 bytes: [a,b,c,d,e,f,_,_] wp=6 len=6
        buf.write(b"ghij"); // 4 bytes: [i,j,c,d,e,f,g,h] wp=2 len=8
                            // Oldest data starts at wp=2 → c,d,e,f,g,h,i,j
        assert_eq!(buf.len(), 8);
        assert_eq!(buf.read_all_string(), "cdefghij");
    }

    #[test]
    fn write_larger_than_capacity() {
        let mut buf = RingBuffer::new(4);
        buf.write(b"abcdefgh");
        assert_eq!(buf.len(), 4);
        assert_eq!(buf.read_all_string(), "efgh");
    }

    #[test]
    fn empty_write_is_noop() {
        let mut buf = RingBuffer::new(8);
        buf.write(b"hello");
        buf.write(b"");
        assert_eq!(buf.read_all_string(), "hello");
    }

    #[test]
    fn tail_lines() {
        let mut buf = RingBuffer::new(256);
        buf.write(b"line1\nline2\nline3\nline4\nline5");
        assert_eq!(buf.read_tail_lines(2), "line4\nline5");
        assert_eq!(buf.read_tail_lines(0), "");
        assert_eq!(buf.read_tail_lines(10), "line1\nline2\nline3\nline4\nline5");
    }
}
