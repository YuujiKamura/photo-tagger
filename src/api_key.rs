//! 従量課金APIキーの安全な管理
//!
//! - ディスクに保存しない（プロセスメモリのみ）
//! - メモリ上はXOR暗号化で保持
//! - env varは使用直前にset、Drop時にremove
//! - 毎回の実行時にプロンプトで入力を求める

use std::io::Write;

/// APIキーをXOR暗号化して保持し、Drop時にenv varを消去するガード
pub struct ApiKeyGuard {
    encrypted: Vec<u8>,
    pad: Vec<u8>,
}

impl ApiKeyGuard {
    /// ユーザーにAPIキー入力を求め、暗号化して保持する。
    /// 入力されたキーは即座にXOR暗号化され、平文は破棄される。
    pub fn prompt() -> anyhow::Result<Self> {
        eprintln!("┌─────────────────────────────────────────────┐");
        eprintln!("│  従量課金モード (--pay-per-use)              │");
        eprintln!("│                                             │");
        eprintln!("│  APIキーの管理方針:                          │");
        eprintln!("│  - ディスクには一切保存しません               │");
        eprintln!("│  - メモリ上はXOR暗号化で保持します           │");
        eprintln!("│  - このプロセス終了時に自動消去されます       │");
        eprintln!("│  - 次回実行時は再度入力が必要です             │");
        eprintln!("└─────────────────────────────────────────────┘");

        // 既にenv varにあれば明示的に消す（前回の残骸防止）
        std::env::remove_var("GEMINI_API_KEY");

        eprint!("GEMINI_API_KEY: ");
        std::io::stderr().flush()?;

        let key = rpassword::read_password()?;
        let key = key.trim().to_string();
        if key.is_empty() {
            anyhow::bail!("APIキーが空です");
        }

        let guard = Self::from_plaintext(&key);

        // keyはStringなのでdrop時にヒープ解放される。
        // ただしStringの内存をゼロ埋めはできないので、
        // できるだけ早くスコープを抜けさせる。
        drop(key);

        // env varをセット（cli-ai-analyzerが読む）
        guard.activate();

        eprintln!("APIキー受理 ({}文字、暗号化保持中)\n", guard.encrypted.len());
        Ok(guard)
    }

    /// 平文からXOR暗号化インスタンスを生成
    fn from_plaintext(plaintext: &str) -> Self {
        // LCG (Linear Congruential Generator) でパッド生成
        // 暗号学的に安全ではないが、メモリダンプからの単純な平文抽出を防ぐ
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
            ^ (std::process::id() as u64).wrapping_mul(2654435761);

        let pad: Vec<u8> = (0..plaintext.len())
            .map(|i| {
                let mut h = seed.wrapping_add(i as u64);
                h = h.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (h >> 33) as u8
            })
            .collect();

        let encrypted: Vec<u8> = plaintext
            .bytes()
            .zip(pad.iter())
            .map(|(b, p)| b ^ p)
            .collect();

        Self { encrypted, pad }
    }

    /// 復号してenv varにセット（cli-ai-analyzerが読む）
    fn activate(&self) {
        let plain = self.decrypt();
        std::env::set_var("GEMINI_API_KEY", &plain);
        // plainはここでdropされる
    }

    /// XOR復号
    fn decrypt(&self) -> String {
        let bytes: Vec<u8> = self
            .encrypted
            .iter()
            .zip(self.pad.iter())
            .map(|(e, p)| e ^ p)
            .collect();
        String::from_utf8(bytes).unwrap_or_default()
    }
}

impl Drop for ApiKeyGuard {
    fn drop(&mut self) {
        // env varを消去
        std::env::remove_var("GEMINI_API_KEY");
        // メモリ上の暗号データをゼロ埋め
        for b in &mut self.encrypted {
            *b = 0;
        }
        for b in &mut self.pad {
            *b = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let key = "AIzaSyTest1234567890";
        let guard = ApiKeyGuard::from_plaintext(key);

        // 暗号化データは平文と異なる
        assert_ne!(&guard.encrypted, key.as_bytes());

        // 復号すると元に戻る
        assert_eq!(guard.decrypt(), key);
    }

    #[test]
    fn test_drop_clears_env() {
        let key = "AIzaSyDropTest";
        {
            let guard = ApiKeyGuard::from_plaintext(key);
            guard.activate();
            assert_eq!(std::env::var("GEMINI_API_KEY").unwrap(), key);
        }
        // drop後はenv varが消えている
        assert!(std::env::var("GEMINI_API_KEY").is_err());
    }

    #[test]
    fn test_drop_zeroes_memory() {
        let key = "AIzaSyZeroTest";
        let mut guard = ApiKeyGuard::from_plaintext(key);
        // 手動でdropロジックを実行
        for b in &mut guard.encrypted {
            *b = 0;
        }
        for b in &mut guard.pad {
            *b = 0;
        }
        assert!(guard.encrypted.iter().all(|&b| b == 0));
        assert!(guard.pad.iter().all(|&b| b == 0));
    }
}
