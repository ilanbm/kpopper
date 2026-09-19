//! Local reference token budgets. Special marker spellings in evidence are text.
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Encoding {
    #[default]
    #[value(name = "o200k_base")]
    O200kBase,
    #[value(name = "cl100k_base")]
    Cl100kBase,
}
impl Encoding {
    pub fn name(self) -> &'static str {
        match self {
            Self::O200kBase => "o200k_base",
            Self::Cl100kBase => "cl100k_base",
        }
    }
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "o200k_base" => Ok(Self::O200kBase),
            "cl100k_base" => Ok(Self::Cl100kBase),
            _ => Err(Error("encoding must be o200k_base or cl100k_base".into())),
        }
    }
    pub fn count(self, text: &str) -> usize {
        let bpe = match self {
            Self::O200kBase => tiktoken_rs::o200k_base_singleton(),
            Self::Cl100kBase => tiktoken_rs::cl100k_base_singleton(),
        };
        bpe.encode_ordinary(text).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reference_encodings_count_markers_as_ordinary_text() {
        for encoding in [Encoding::O200kBase, Encoding::Cl100kBase] {
            assert_eq!(encoding.count(""), 0);
            assert_eq!(encoding.count("hello world"), 2);
            assert!(encoding.count("<|endoftext|>") > 1);
            assert!(encoding.count("שלום עולם") > 0);
            assert_eq!(Encoding::parse(encoding.name()).unwrap(), encoding);
        }
        assert!(Encoding::parse("chars").is_err());
    }
}
