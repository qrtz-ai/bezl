//! The candle MiniLM embedder (ADR 0050). Without weights (CI): the pipeline on a tiny random BERT built here
//! (padding never changes a vector, unit length, batch = one by one, hash pinning). With `QRTZ_MINILM_DIR` set to
//! the pinned all-MiniLM-L6-v2 files: the vectors match the Electron worker's fp32 output to 1e-5 per dimension and
//! its shipped q8 output to cosine > 0.93 (quantization noise; measured 0.955-0.992), spec/vectors/search/minilm.json.
#![cfg(feature = "minilm")]
use std::path::Path;

use candle_core::{DType, Device};
use candle_nn::{VarBuilder, VarMap};
use candle_transformers::models::bert::{BertModel, Config};
use qrtz_search::minilm::{verify_dir, MiniLm, MODEL_ID};
use qrtz_search::Embedder;
use serde_json::{json, Value};

fn tiny_model(dir: &Path) {
    let config = json!({
        "vocab_size": 40, "hidden_size": 16, "num_hidden_layers": 2, "num_attention_heads": 2,
        "intermediate_size": 32, "hidden_act": "gelu", "hidden_dropout_prob": 0.0, "max_position_embeddings": 64,
        "type_vocab_size": 2, "initializer_range": 0.02, "layer_norm_eps": 1e-12, "pad_token_id": 0,
        "position_embedding_type": "absolute", "model_type": "bert"
    });
    std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
    let cfg: Config = serde_json::from_value(config).unwrap();
    let vm = VarMap::new();
    let vb = VarBuilder::from_varmap(&vm, DType::F32, &Device::Cpu);
    BertModel::load(vb, &cfg).unwrap();
    vm.save(dir.join("model.safetensors")).unwrap();
    let mut vocab = serde_json::Map::new();
    let words = [
        "[PAD]", "[UNK]", "[CLS]", "[SEP]", "the", "budget", "is", "approved", "launch", "on", "friday",
        "we", "decided", "to", "delay", "##s", "a", "b", "c", ".", ",", "?",
    ];
    for (i, w) in words.iter().enumerate() {
        vocab.insert((*w).into(), json!(i));
    }
    let special = |t: &str, id: usize| json!({"id": t, "ids": [id], "tokens": [t]});
    let added: Vec<Value> = [0usize, 1, 2, 3]
        .iter()
        .map(|&i| {
            json!({"id": i, "content": words[i], "single_word": false, "lstrip": false, "rstrip": false,
            "normalized": false, "special": true})
        })
        .collect();
    let tok = json!({
        "version": "1.0", "truncation": null, "padding": null,
        "added_tokens": added,
        "normalizer": {"type": "BertNormalizer", "clean_text": true, "handle_chinese_chars": true,
            "strip_accents": null, "lowercase": true},
        "pre_tokenizer": {"type": "BertPreTokenizer"},
        "post_processor": {"type": "TemplateProcessing",
            "single": [{"SpecialToken": {"id": "[CLS]", "type_id": 0}}, {"Sequence": {"id": "A", "type_id": 0}},
                {"SpecialToken": {"id": "[SEP]", "type_id": 0}}],
            "pair": [{"Sequence": {"id": "A", "type_id": 0}}, {"Sequence": {"id": "B", "type_id": 1}}],
            "special_tokens": {"[CLS]": special("[CLS]", 2), "[SEP]": special("[SEP]", 3)}},
        "decoder": null,
        "model": {"type": "WordPiece", "unk_token": "[UNK]", "continuing_subword_prefix": "##",
            "max_input_chars_per_word": 100, "vocab": vocab}
    });
    std::fs::write(dir.join("tokenizer.json"), tok.to_string()).unwrap();
}

fn norm(v: &[f32]) -> f64 {
    v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt()
}

#[tokio::test]
async fn tiny_model_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    tiny_model(dir.path());
    let m = MiniLm::load_unverified(dir.path(), "tiny").unwrap();
    assert_eq!(m.id(), "tiny");
    let texts: Vec<String> = [
        "the budget is approved",
        "we decided to delay the launch on friday.",
        "a",
        "",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let batch = m.embed(&texts).await.unwrap();
    assert_eq!(batch.len(), 4);
    for (t, v) in texts.iter().zip(&batch) {
        assert_eq!(v.len(), 16);
        assert!((norm(v) - 1.0).abs() < 1e-5, "{t}");
        // Padding in the batch never changes a vector.
        let one = m.embed(std::slice::from_ref(t)).await.unwrap();
        let d = v
            .iter()
            .zip(&one[0])
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(d < 1e-5, "{t}: {d}");
    }
    assert_ne!(batch[0], batch[1]);
    assert!(m.embed(&[]).await.unwrap().is_empty());
    // Not the pinned files: refused by load (the app then stays full-text only).
    assert!(verify_dir(dir.path())
        .unwrap_err()
        .starts_with("config.json: sha256"));
    assert!(MiniLm::load(dir.path()).is_err());
}

#[tokio::test]
async fn matches_the_electron_vectors_with_the_pinned_weights() {
    let Ok(dir) = std::env::var("QRTZ_MINILM_DIR") else {
        eprintln!("QRTZ_MINILM_DIR not set: skipping the real-model parity check");
        return;
    };
    let m = MiniLm::load(Path::new(&dir)).unwrap();
    assert_eq!(m.id(), MODEL_ID);
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors/search/minilm.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let texts: Vec<String> = serde_json::from_value(v["texts"].clone()).unwrap();
    let got = m.embed(&texts).await.unwrap();
    let want = |k: &str| -> Vec<Vec<f32>> { serde_json::from_value(v[k].clone()).unwrap() };
    let cos =
        |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum::<f64>() / norm(b);
    for (i, (g, (f32w, q8w))) in got.iter().zip(want("fp32").iter().zip(&want("q8"))).enumerate() {
        let max_abs = g
            .iter()
            .zip(f32w)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        let (c32, c8) = (cos(g, f32w), cos(g, q8w));
        eprintln!("{i}: max|Δ| vs fp32 {max_abs:.2e}, cos fp32 {c32:.6}, cos q8 {c8:.4}");
        assert!(
            max_abs < 1e-5 && c32 > 0.999999,
            "text {i} vs fp32: {max_abs} {c32}"
        );
        assert!(c8 > 0.93, "text {i} vs q8: {c8}");
    }
}

/// `download` against a local mock server: bytes that don't match the pinned hashes are refused and nothing is
/// written; an HTTP error is reported. (`--features minilm-download`; never the real network.)
#[cfg(feature = "minilm-download")]
#[tokio::test]
async fn download_refuses_unpinned_bytes() {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        for (n, s) in listener.incoming().enumerate() {
            let mut s = s.unwrap();
            let mut line = String::new();
            let mut r = BufReader::new(s.try_clone().unwrap());
            r.read_line(&mut line).unwrap();
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
            }
            seen2.lock().unwrap().push(line.trim().to_string());
            let reply = if n == 0 {
                "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}".to_string()
            } else {
                "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string()
            };
            s.write_all(reply.as_bytes()).unwrap();
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let client = reqwest::Client::new();
    let e = qrtz_search::minilm::download(&client, &base, dir.path())
        .await
        .unwrap_err();
    assert!(e.starts_with("config.json: sha256 44136fa3"), "{e}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    let e = qrtz_search::minilm::download(&client, &base, dir.path())
        .await
        .unwrap_err();
    assert_eq!(e, "config.json: HTTP 404 Not Found");
    assert_eq!(
        seen.lock().unwrap()[0],
        format!(
            "GET /{}/resolve/{}/config.json HTTP/1.1",
            qrtz_search::minilm::REPO,
            qrtz_search::minilm::REVISION
        )
    );
}
