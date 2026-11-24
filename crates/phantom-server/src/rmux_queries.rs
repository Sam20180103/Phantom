// rmux already answers DA and DSR. Forwarding them makes each viewer answer
// again, including when replaying queries from an earlier session.
#[derive(Default)]
pub struct QueryFilter {
    pending: Vec<u8>,
}

impl QueryFilter {
    pub fn process(&mut self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len());
        for &byte in input {
            if self.pending.is_empty() {
                if byte == 0x1b {
                    self.pending.push(byte);
                } else {
                    out.push(byte);
                }
                continue;
            }
            if byte == 0x1b {
                out.append(&mut self.pending);
                self.pending.push(byte);
                continue;
            }
            self.pending.push(byte);
            if self.pending.len() == 2 && byte == b'[' {
                continue;
            }
            let csi = self.pending.get(1) == Some(&b'[');
            if !csi || (0x40..=0x7e).contains(&byte) || self.pending.len() >= 256 {
                let query = matches!(
                    self.pending.as_slice(),
                    b"\x1b[c" | b"\x1b[0c" | b"\x1b[>c" | b"\x1b[>0c" | b"\x1b[5n" | b"\x1b[6n"
                );
                if !query {
                    out.append(&mut self.pending);
                }
                self.pending.clear();
            }
        }
        out
    }

    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

#[cfg(test)]
mod tests {
    use super::QueryFilter;

    #[test]
    fn queries_are_removed_at_every_chunk_boundary_without_changing_rendering() {
        let input = b"hello\x1b[c\x1b[0c\x1b[>c\x1b[>0c\x1b[5n\x1b[6n\x1b[2J\x1b[12;4Hworld\x1b]11;?\x07\x1b[?1;2c";
        let expected = b"hello\x1b[2J\x1b[12;4Hworld\x1b]11;?\x07\x1b[?1;2c";
        for boundary in 0..=input.len() {
            let mut filter = QueryFilter::default();
            let mut out = filter.process(&input[..boundary]);
            out.extend(filter.process(&input[boundary..]));
            out.extend(filter.finish());
            assert_eq!(out, expected, "boundary {boundary}");
        }
        let mut filter = QueryFilter::default();
        let mut out = Vec::new();
        for byte in input {
            out.extend(filter.process(&[*byte]));
        }
        out.extend(filter.finish());
        assert_eq!(out, expected);
    }
}
