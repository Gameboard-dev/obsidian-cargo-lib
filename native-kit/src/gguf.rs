//! Qwen3 weights as a GGUF file, which candle's quantized Qwen3 reads
//! (<https://docs.rs/candle-transformers/latest/candle_transformers/models/quantized_qwen3/>; the format:
//! <https://github.com/ggml-org/ggml/blob/master/docs/gguf.md>). Qwen publishes no GGUF for its embedding and reranking
//! models, so the official safetensors are quantized once into the server's cache. The same names and metadata as
//! Augmented View's claims.rs, which can use this.
//!
//! `head`: what the output layer becomes. `Tied` keeps the language model's (the embedding matrix); `Identity` makes it
//! the identity, so the model's output is the last token's hidden state after the final norm: an embedding model's
//! last-token pooling (Qwen3-Embedding) without changing candle. `Rows` keeps only some rows of the output layer (a
//! reranker's "yes" and "no" tokens).

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use candle_core::quantized::{gguf_file, GgmlDType, QTensor};
use candle_core::{safetensors::MmapedSafetensors, DType, Device, Result, Tensor};
use serde_json::Value;

pub enum Head {
    Tied,
    Identity,
    Rows(Vec<u32>),
}

/// candle's names for the official weights' (`model.layers.N.self_attn.q_proj.weight` → `blk.N.attn_q.weight`).
fn rename(n: &str) -> Option<String> {
    let n = n.strip_prefix("model.").unwrap_or(n);
    let fixed = [("embed_tokens.weight", "token_embd.weight"), ("norm.weight", "output_norm.weight"), ("lm_head.weight", "output.weight")];
    if let Some((_, g)) = fixed.iter().find(|(h, _)| *h == n) {
        return Some(g.to_string());
    }
    let (layer, rest) = n.strip_prefix("layers.")?.split_once('.')?;
    let part = [
        ("input_layernorm", "attn_norm"),
        ("post_attention_layernorm", "ffn_norm"),
        ("self_attn.q_proj", "attn_q"),
        ("self_attn.k_proj", "attn_k"),
        ("self_attn.v_proj", "attn_v"),
        ("self_attn.o_proj", "attn_output"),
        ("self_attn.q_norm", "attn_q_norm"),
        ("self_attn.k_norm", "attn_k_norm"),
        ("mlp.gate_proj", "ffn_gate"),
        ("mlp.up_proj", "ffn_up"),
        ("mlp.down_proj", "ffn_down"),
    ]
    .iter()
    .find(|(h, _)| rest == format!("{h}.weight"))?
    .1;
    Some(format!("blk.{layer}.{part}.weight"))
}

/// Write `out` from the safetensors `shards` and the model's config.json (`cfg`): matrices as `quant`, norms in f32,
/// activations in f32 (`general.dtype` 0; better for embeddings than f16, and what the CPU runs fastest).
pub fn convert(shards: &[std::path::PathBuf], cfg: &Value, out: &Path, quant: GgmlDType, head: Head, context: u32) -> Result<()> {
    let st = unsafe { MmapedSafetensors::multi(shards)? };
    let mut tensors: Vec<(String, QTensor)> = Vec::new();
    let mut embed: Option<Tensor> = None;
    for (name, _) in st.tensors() {
        let Some(g) = rename(&name) else { continue };
        let t = st.load(&name, &Device::Cpu)?.to_dtype(DType::F32)?;
        if g == "token_embd.weight" {
            embed = Some(t.clone());
        }
        if g == "output.weight" && !matches!(head, Head::Tied) {
            continue;
        }
        let dtype = if t.rank() == 1 { GgmlDType::F32 } else { quant };
        tensors.push((g, QTensor::quantize(&t, dtype)?));
    }
    let hidden = cfg["hidden_size"].as_u64().unwrap_or(0) as usize;
    match head {
        Head::Tied => {}
        Head::Identity => tensors.push(("output.weight".into(), QTensor::quantize(&Tensor::eye(hidden, DType::F32, &Device::Cpu)?, GgmlDType::F32)?)),
        Head::Rows(ids) => {
            let w = match st.load("lm_head.weight", &Device::Cpu) {
                Ok(w) => w.to_dtype(DType::F32)?,
                Err(_) => embed.clone().ok_or_else(|| candle_core::Error::Msg("no embedding matrix".into()))?,
            };
            let rows = w.index_select(&Tensor::new(ids.as_slice(), &Device::Cpu)?, 0)?;
            tensors.push(("output.weight".into(), QTensor::quantize(&rows, GgmlDType::F32)?));
        }
    }
    let u = |k: &str| gguf_file::Value::U32(cfg[k].as_u64().unwrap_or(0) as u32);
    let f = |k: &str| gguf_file::Value::F32(cfg[k].as_f64().unwrap_or(0.0) as f32);
    let meta = [
        ("general.architecture", gguf_file::Value::String("qwen3".into())),
        ("general.dtype", gguf_file::Value::U32(0)),
        ("qwen3.attention.head_count", u("num_attention_heads")),
        ("qwen3.attention.head_count_kv", u("num_key_value_heads")),
        ("qwen3.attention.key_length", u("head_dim")),
        ("qwen3.block_count", u("num_hidden_layers")),
        ("qwen3.embedding_length", u("hidden_size")),
        ("qwen3.context_length", gguf_file::Value::U32(context)),
        ("qwen3.attention.layer_norm_rms_epsilon", f("rms_norm_eps")),
        ("qwen3.rope.freq_base", f("rope_theta")),
    ];
    std::fs::create_dir_all(out.parent().unwrap_or(Path::new(".")))?;
    let tmp = out.with_extension("part");
    let mut w = BufWriter::new(File::create(&tmp)?);
    gguf_file::write(&mut w, &meta.iter().map(|(k, v)| (*k, v)).collect::<Vec<_>>(), &tensors.iter().map(|(k, v)| (k.as_str(), v)).collect::<Vec<_>>())?;
    drop(w);
    std::fs::rename(tmp, out)?;
    Ok(())
}

/// The safetensors shards of a model (`model.safetensors`, or those its index names).
pub fn shards(cache: &Path, repo: &str) -> std::result::Result<Vec<std::path::PathBuf>, String> {
    match crate::models::need(cache, repo, "model.safetensors.index.json") {
        Ok(index) => {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(index).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let mut files: Vec<String> = v["weight_map"].as_object().into_iter().flatten().filter_map(|(_, f)| f.as_str().map(String::from)).collect();
            files.sort();
            files.dedup();
            files.iter().map(|f| crate::models::need(cache, repo, f)).collect()
        }
        Err(_) => Ok(vec![crate::models::need(cache, repo, "model.safetensors")?]),
    }
}
