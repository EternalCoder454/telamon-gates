//! What a GGUF model file says about itself: its name, size label and
//! quantisation, read from the header's metadata. Model files come from
//! anywhere: every length is bounded and nothing is trusted to fit memory.
//!
//! Reads a file: call it from a worker thread.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// What the header says; empty fields when it doesn't.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    /// `general.name`.
    pub name: String,
    /// `general.size_label` ("8B", "27B", "26B-A4B").
    pub size_label: String,
    /// The quantisation ("Q4_K_M"), from `general.file_type`, else the file
    /// name.
    pub quant: String,
    /// A decision model's kind (`<arch>.decision.type`: "laya", "kev", …),
    /// which answers typed questions through `/v1/systemone` and writes no
    /// text; "" for a model that chats.
    pub decision: String,
    /// Its chat template takes tools (Agent mode can use it).
    pub tools: bool,
    /// The context it was trained for, in tokens (`<arch>.context_length`);
    /// 0 when it doesn't say.
    pub context_length: u32,
    /// `general.architecture` ("llama", "qwen3", "gemma4"); "" when it
    /// doesn't say.
    pub architecture: String,
    /// The number of layers (`<arch>.block_count`); 0 when it doesn't say.
    /// A failed load steps the layers on the graphics card down from it.
    pub block_count: u32,
}

/// llama.cpp tag the packaged `llama-server` is built from (`packaging/
/// telamon-llama/telamon-llama.spec` `Version`); `SUPPORTED_ARCHITECTURES` is
/// its `src/llama-arch.cpp` `LLM_ARCH_NAMES`, less the `clip` projector
/// and `(unknown)`. Update both with the package.
pub const LLAMA_CPP_TAG: &str = "v0.6.0";

/// The `general.architecture` values telamon-llama's build loads.
pub const SUPPORTED_ARCHITECTURES: &[&str] = &[
    "llama",
    "llama4",
    "deci",
    "falcon",
    "grok",
    "gpt2",
    "gptj",
    "gptneox",
    "mpt",
    "baichuan",
    "starcoder",
    "refact",
    "bert",
    "modern-bert",
    "nomic-bert",
    "nomic-bert-moe",
    "neo-bert",
    "jina-bert-v2",
    "jina-bert-v3",
    "eurobert",
    "bloom",
    "stablelm",
    "qwen",
    "qwen2",
    "qwen2moe",
    "qwen2vl",
    "qwen3",
    "qwen3moe",
    "qwen3next",
    "qwen3vl",
    "qwen3vlmoe",
    "qwen35",
    "qwen35moe",
    "clef",
    "qwen4exp",
    "phi2",
    "phi3",
    "phimoe",
    "plamo",
    "plamo2",
    "plamo3",
    "codeshell",
    "orion",
    "internlm2",
    "minicpm",
    "minicpm3",
    "gemma",
    "gemma2",
    "gemma3",
    "gemma3n",
    "gemma4",
    "gemma4-assistant",
    "gemma-embedding",
    "starcoder2",
    "mamba",
    "mamba2",
    "maple",
    "jamba",
    "falcon-h1",
    "xverse",
    "command-r",
    "cohere2",
    "cohere2moe",
    "dbrx",
    "olmo",
    "olmo2",
    "olmoe",
    "muse-glimmer",
    "openelm",
    "arctic",
    "deepseek",
    "deepseek2",
    "deepseek2-ocr",
    "deepseek32",
    "deepseek4",
    "chatglm",
    "glm4",
    "glm4moe",
    "glm-dsa",
    "bitnet",
    "t5",
    "t5encoder",
    "jais",
    "jais2",
    "nemotron",
    "nemotron_h",
    "nemotron_h_moe",
    "exaone",
    "exaone4",
    "exaone-moe",
    "rwkv6",
    "rwkv6qwen2",
    "rwkv7",
    "arwkv7",
    "granite",
    "granitemoe",
    "granitehybrid",
    "graniteswitch",
    "granite_swa",
    "chameleon",
    "wavtokenizer-dec",
    "plm",
    "bailingmoe",
    "bailingmoe2",
    "bailingmoe3",
    "dots1",
    "dots3note",
    "arcee",
    "afmoe",
    "laguna",
    "ernie4_5",
    "ernie4_5-moe",
    "hunyuan-moe",
    "hunyuan-dense",
    "hunyuan_vl",
    "hy_v3",
    "hy_v4",
    "smollm3",
    "gpt-oss",
    "lfm2",
    "lfm2moe",
    "dream",
    "smallthinker",
    "llada",
    "llada-moe",
    "seed_oss",
    "grovemoe",
    "apertus",
    "minimax-01",
    "hrm_text",
    "minimax-m2",
    "minimax-m3",
    "cogvlm",
    "rnd1",
    "pangu-embedded",
    "mistral3",
    "eagle3",
    "dflash",
    "mistral4",
    "paddleocr",
    "mimo2",
    "step35",
    "spark2_5",
    "llama-embed",
    "maincoder",
    "kimi-linear",
    "kimi-k3",
    "glm5-next",
    "talkie",
    "mellum",
    "nanbeige",
    "qwen3tts",
    "pockettts",
];

/// Whether the packaged llama-server loads a model of architecture `arch`.
/// An empty one (the header doesn't say, or it isn't GGUF) counts as
/// supported: llama.cpp reports what is wrong with the file itself.
pub fn is_supported(arch: &str) -> bool {
    arch.is_empty() || SUPPORTED_ARCHITECTURES.contains(&arch)
}

impl Info {
    /// Whether llama-server can load this model (see `is_supported`).
    pub fn supported(&self) -> bool {
        is_supported(&self.architecture)
    }
}

/// The longest string read whole; longer ones are skipped.
const MAX_STRING: u64 = 64 * 1024;
/// The most metadata entries looked at.
const MAX_KEYS: u64 = 4096;

/// Reads `path`'s header. Errors only when the file can't be opened or isn't
/// GGUF; damaged metadata gives what was read before it.
pub fn read(path: &Path) -> io::Result<Info> {
    let mut r = BufReader::new(File::open(path)?);
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a GGUF file",
        ));
    }
    let version = u32_le(&mut r)?;
    if !(2..=3).contains(&version) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unknown GGUF version",
        ));
    }
    let _tensors = u64_le(&mut r)?;
    let keys = u64_le(&mut r)?.min(MAX_KEYS);
    let mut info = Info::default();
    let mut file_type = None;
    // `<arch>.context_length` and `.block_count`, once the architecture is
    // known (it comes first).
    let mut context_key: Option<String> = None;
    let mut block_key: Option<String> = None;
    for _ in 0..keys {
        let Ok(key) = string(&mut r) else { break };
        let Ok(kind) = u32_le(&mut r) else { break };
        let wanted = matches!(
            key.as_deref(),
            Some(
                "general.name"
                    | "general.size_label"
                    | "general.file_type"
                    | "general.architecture"
            )
        ) || key.as_deref().is_some_and(|k| {
            k.ends_with(".decision.type")
                || k == "tokenizer.chat_template"
                || k.ends_with(".context_length")
                || k.ends_with(".block_count")
        });
        if !wanted {
            if skip_value(&mut r, kind, 0).is_err() {
                break;
            }
            continue;
        }
        match (key.as_deref(), kind) {
            (Some("general.file_type"), 4) => file_type = u32_le(&mut r).ok(),
            (Some(k), 4) if Some(k) == context_key.as_deref() => {
                info.context_length = u32_le(&mut r).unwrap_or(0);
            }
            (Some(k), 10) if Some(k) == context_key.as_deref() => {
                info.context_length =
                    u64_le(&mut r).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
            }
            (Some(k), 4) if Some(k) == block_key.as_deref() => {
                info.block_count = u32_le(&mut r).unwrap_or(0);
            }
            (Some(k), 10) if Some(k) == block_key.as_deref() => {
                info.block_count =
                    u64_le(&mut r).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
            }
            (Some(k), 8) => {
                let Ok(value) = string(&mut r) else { break };
                let value = value.unwrap_or_default();
                if k == "general.name" {
                    info.name = value;
                } else if k == "general.size_label" {
                    info.size_label = value;
                } else if k == "general.architecture" {
                    context_key = Some(format!("{value}.context_length"));
                    block_key = Some(format!("{value}.block_count"));
                    info.architecture = value;
                } else if k.ends_with(".decision.type") {
                    info.decision = value;
                } else if k == "tokenizer.chat_template" {
                    info.tools = value.contains("tools");
                }
            }
            _ => {
                if skip_value(&mut r, kind, 0).is_err() {
                    break;
                }
            }
        }
    }
    info.quant = file_type
        .and_then(quant_name)
        .map(str::to_string)
        .unwrap_or_else(|| quant_from_name(&path.to_string_lossy()));
    Ok(info)
}

fn u32_le(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn u64_le(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

/// A GGUF string: its text when short enough (lossy UTF-8), else None (and
/// skipped).
fn string<R: Read + Seek>(r: &mut R) -> io::Result<Option<String>> {
    let len = u64_le(r)?;
    if len > MAX_STRING {
        skip(r, len)?;
        return Ok(None);
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

fn skip<R: Seek>(r: &mut R, n: u64) -> io::Result<()> {
    let n = i64::try_from(n).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "length"))?;
    r.seek(SeekFrom::Current(n))?;
    Ok(())
}

/// Skips a value of GGUF type `kind`; arrays of strings are walked (each has
/// its length), others jumped over. Nested arrays at most 2 deep.
fn skip_value<R: Read + Seek>(r: &mut R, kind: u32, depth: u32) -> io::Result<()> {
    let fixed = match kind {
        0 | 1 | 7 => Some(1u64), // u8, i8, bool
        2 | 3 => Some(2),        // u16, i16
        4..=6 => Some(4),        // u32, i32, f32
        10..=12 => Some(8),      // u64, i64, f64
        _ => None,
    };
    if let Some(size) = fixed {
        return skip(r, size);
    }
    match kind {
        8 => {
            let len = u64_le(r)?;
            skip(r, len)
        }
        9 if depth < 2 => {
            let element = u32_le(r)?;
            let count = u64_le(r)?;
            let size = match element {
                0 | 1 | 7 => Some(1u64),
                2 | 3 => Some(2),
                4..=6 => Some(4),
                10..=12 => Some(8),
                _ => None,
            };
            match size {
                Some(size) => skip(
                    r,
                    count
                        .checked_mul(size)
                        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "array"))?,
                ),
                None => {
                    for _ in 0..count {
                        skip_value(r, element, depth + 1)?;
                    }
                    Ok(())
                }
            }
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unknown value type",
        )),
    }
}

/// llama.cpp's `llama_ftype` numbers, as people name them.
fn quant_name(file_type: u32) -> Option<&'static str> {
    Some(match file_type {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4",
        _ => return None,
    })
}

/// The quantisation a file name carries ("…-Q4_K_M.gguf", "….IQ3_M.gguf",
/// "…-UD-Q4_K_XL.gguf"); "" when none.
pub fn quant_from_name(name: &str) -> String {
    let stem = name
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .trim_end_matches(".gguf")
        .to_ascii_uppercase();
    stem.split(['-', '.'])
        .rev()
        .find(|part| {
            let p = *part;
            (p.starts_with('Q') || p.starts_with("IQ") || p.starts_with("TQ"))
                && p.chars()
                    .nth(if p.starts_with('Q') { 1 } else { 2 })
                    .is_some_and(|c| c.is_ascii_digit())
                || p == "F16"
                || p == "BF16"
                || p == "F32"
        })
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A GGUF header with the given metadata (and no tensors).
    fn header(entries: &[(&str, u32, Vec<u8>)]) -> Vec<u8> {
        let mut out = b"GGUF".to_vec();
        out.extend(3u32.to_le_bytes());
        out.extend(0u64.to_le_bytes());
        out.extend((entries.len() as u64).to_le_bytes());
        for (key, kind, value) in entries {
            out.extend((key.len() as u64).to_le_bytes());
            out.extend(key.as_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(value);
        }
        out
    }

    fn gguf_string(s: &str) -> Vec<u8> {
        let mut v = (s.len() as u64).to_le_bytes().to_vec();
        v.extend(s.as_bytes());
        v
    }

    fn write(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("gates-gguf-{}-{name}", std::process::id()));
        File::create(&path).unwrap().write_all(bytes).unwrap();
        path
    }

    #[test]
    fn reads_the_name_size_and_quant() {
        // A string array (a vocabulary) before the wanted keys, to skip.
        let mut vocab = 8u32.to_le_bytes().to_vec();
        vocab.extend(3u64.to_le_bytes());
        for w in ["a", "bb", "ccc"] {
            vocab.extend(gguf_string(w));
        }
        let bytes = header(&[
            ("general.architecture", 8, gguf_string("llama")),
            ("tokenizer.ggml.tokens", 9, vocab),
            ("llama.context_length", 4, 8192u32.to_le_bytes().to_vec()),
            ("llama.block_count", 4, 32u32.to_le_bytes().to_vec()),
            ("general.name", 8, gguf_string("Tiny Model")),
            ("general.size_label", 8, gguf_string("135M")),
            ("general.file_type", 4, 15u32.to_le_bytes().to_vec()),
        ]);
        let path = write("ok.gguf", &bytes);
        let info = read(&path).unwrap();
        assert_eq!(
            info,
            Info {
                name: "Tiny Model".into(),
                size_label: "135M".into(),
                quant: "Q4_K_M".into(),
                decision: String::new(),
                tools: false,
                context_length: 8192,
                architecture: "llama".into(),
                block_count: 32,
            }
        );
        let _ = std::fs::remove_file(path);
        // A template that takes tools.
        let bytes = header(&[(
            "tokenizer.chat_template",
            8,
            gguf_string("{% if tools %}<tools>{{ tools }}</tools>{% endif %}"),
        )]);
        let path = write("tools.gguf", &bytes);
        assert!(read(&path).unwrap().tools);
        let _ = std::fs::remove_file(path);
        // A decision model says so under its architecture's name.
        let bytes = header(&[
            ("general.architecture", 8, gguf_string("modern-bert")),
            ("modern-bert.decision.type", 8, gguf_string("laya")),
        ]);
        let path = write("laya.gguf", &bytes);
        assert_eq!(read(&path).unwrap().decision, "laya");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn damaged_or_foreign_files() {
        let not = write("not.gguf", b"PK\x03\x04 a zip");
        assert!(read(&not).is_err());
        // Cut off in the middle of its metadata: what was read before stays,
        // and the quant comes from the file name.
        let mut cut = header(&[("general.name", 8, gguf_string("Cut"))]);
        cut.extend(5u64.to_le_bytes()); // a key length, then nothing
        let path = write("Model-IQ3_M.gguf", &cut);
        let info = read(&path).unwrap();
        assert_eq!(info.name, "Cut");
        assert_eq!(info.quant, "IQ3_M");
        // A huge declared string is skipped, not allocated.
        let mut huge = b"GGUF".to_vec();
        huge.extend(3u32.to_le_bytes());
        huge.extend(0u64.to_le_bytes());
        huge.extend(1u64.to_le_bytes());
        huge.extend(u64::MAX.to_le_bytes());
        let path2 = write("huge.gguf", &huge);
        assert!(read(&path2).is_ok());
        for p in [not, path, path2] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn architectures_the_server_loads() {
        // Read from the models Gates is tested with: Kev is qwen35, Laya
        // modern-bert, a DSpark draft dflash.
        for arch in [
            "llama",
            "qwen3",
            "qwen3moe",
            "qwen35",
            "gemma4",
            "gpt-oss",
            "modern-bert",
            "dflash",
            "eagle3",
        ] {
            assert!(is_supported(arch), "{arch}");
        }
        for arch in ["clip", "(unknown)", "mamba3000", "Llama", "qwen 3", "x/y"] {
            assert!(!is_supported(arch), "{arch}");
        }
        // The header doesn't say: llama.cpp explains the file.
        assert!(is_supported(""));
        assert!(Info::default().supported());
        // Read from a file, and a made-up one is caught.
        let path = write(
            "future.gguf",
            &header(&[("general.architecture", 8, gguf_string("future-arch"))]),
        );
        let info = read(&path).unwrap();
        assert_eq!(info.architecture, "future-arch");
        assert!(!info.supported());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_architecture_list_matches_the_packaged_server() {
        // Bumping telamon-llama means checking `SUPPORTED_ARCHITECTURES`
        // against the new tag's src/llama-arch.cpp.
        let spec = include_str!("../../../packaging/telamon-llama/telamon-llama.spec");
        let version = spec
            .lines()
            .find_map(|l| l.strip_prefix("Version:"))
            .expect("a Version line")
            .trim();
        assert_eq!(format!("v{version}"), LLAMA_CPP_TAG);
        let mut sorted = SUPPORTED_ARCHITECTURES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), SUPPORTED_ARCHITECTURES.len(), "duplicates");
    }

    #[test]
    fn quants_in_file_names() {
        assert_eq!(quant_from_name("Qwen3.5-9B-Q4_K_M.gguf"), "Q4_K_M");
        assert_eq!(quant_from_name("gemma-4-31b-it-UD-Q4_K_XL.gguf"), "Q4_K_XL");
        assert_eq!(quant_from_name("model.IQ4_NL.gguf"), "IQ4_NL");
        assert_eq!(quant_from_name("SmolLM2-135M-BF16.gguf"), "BF16");
        assert_eq!(quant_from_name("plain.gguf"), "");
    }
}
