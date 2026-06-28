#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ClassDefLayout {
    nbases: usize,
    nattrs: usize,
    layout_size: i64,
    layout_version: i64,
    flags: i64,
}

impl ClassDefLayout {
    pub(super) fn parse(meta: &str) -> Self {
        let mut parts = meta.split(',');
        let nbases = parts
            .next()
            .and_then(|s| s.parse::<usize>().ok())
            .expect("class_def metadata missing base count");
        let nattrs = parts
            .next()
            .and_then(|s| s.parse::<usize>().ok())
            .expect("class_def metadata missing attr count");
        let layout_size = parts
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .expect("class_def metadata missing layout size");
        let layout_version = parts
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .expect("class_def metadata missing layout version");
        let flags = parts
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .expect("class_def metadata missing flags");
        Self {
            nbases,
            nattrs,
            layout_size,
            layout_version,
            flags,
        }
    }

    pub(super) const fn nbases(self) -> usize {
        self.nbases
    }

    pub(super) const fn nattrs(self) -> usize {
        self.nattrs
    }

    pub(super) const fn layout_size(self) -> i64 {
        self.layout_size
    }

    pub(super) const fn layout_version(self) -> i64 {
        self.layout_version
    }

    pub(super) const fn flags(self) -> i64 {
        self.flags
    }

    pub(super) fn spill_words(self) -> usize {
        self.base_words() + self.attr_words()
    }

    pub(super) fn base_words(self) -> usize {
        self.nbases.max(1)
    }

    fn attr_words(self) -> usize {
        (self.nattrs * 2).max(1)
    }

    pub(super) fn attrs_start_arg_index(self) -> usize {
        1 + self.nbases
    }

    pub(super) fn attrs_base_offset(self, spill_base: u32) -> u32 {
        spill_base + (self.base_words() as u32) * 8
    }
}

#[cfg(test)]
mod tests {
    use super::ClassDefLayout;

    #[test]
    fn class_def_layout_parses_full_metadata_once() {
        let layout = ClassDefLayout::parse("2,3,24,7,5");

        assert_eq!(layout.nbases(), 2);
        assert_eq!(layout.nattrs(), 3);
        assert_eq!(layout.layout_size(), 24);
        assert_eq!(layout.layout_version(), 7);
        assert_eq!(layout.flags(), 5);
        assert_eq!(layout.attrs_start_arg_index(), 3);
        assert_eq!(layout.attrs_base_offset(100), 116);
        assert_eq!(layout.spill_words(), 8);
    }

    #[test]
    fn class_def_layout_reserves_minimum_base_and_attr_words() {
        let layout = ClassDefLayout::parse("0,0,0,1,0");

        assert_eq!(layout.base_words(), 1);
        assert_eq!(layout.spill_words(), 2);
        assert_eq!(layout.attrs_base_offset(64), 72);
    }
}
