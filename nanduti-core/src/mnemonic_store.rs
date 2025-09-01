//! Secure mnemonic storage with encryption

use anyhow::{Context, Result};
use fedimint_bip39::Mnemonic;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Mnemonic storage operations
pub struct MnemonicStore;

impl MnemonicStore {
    /// Get the mnemonic file path
    fn get_mnemonic_path(base_path: &Path) -> PathBuf {
        base_path.join(".mnemonic")
    }

    /// Store a mnemonic with password encryption
    pub async fn store_mnemonic(
        base_path: &Path,
        mnemonic: &Mnemonic,
        password: Option<&str>,
    ) -> Result<()> {
        let path = Self::get_mnemonic_path(base_path);

        // Generate salt
        let salt: [u8; 32] = rand::random();

        // Derive key from password
        let key = Self::derive_key(password, &salt);

        // Convert mnemonic to bytes
        let mnemonic_str = mnemonic.to_string();
        let mnemonic_bytes = mnemonic_str.as_bytes();

        // Simple XOR encryption
        let encrypted_data = Self::xor_encrypt(mnemonic_bytes, &key);

        // Combine salt and encrypted data
        let mut file_content = Vec::new();
        file_content.extend_from_slice(&salt);
        file_content.extend_from_slice(&encrypted_data);

        // Write to file
        tokio::fs::write(&path, file_content)
            .await
            .context("Failed to write mnemonic file")?;

        Ok(())
    }

    /// Load mnemonic with password decryption
    pub async fn load_mnemonic(
        base_path: &Path,
        password: Option<&str>,
    ) -> Result<Option<Mnemonic>> {
        let path = Self::get_mnemonic_path(base_path);

        // Check if file exists
        if !path.exists() {
            return Ok(None);
        }

        // Read file
        let file_content = tokio::fs::read(&path)
            .await
            .context("Failed to read mnemonic file")?;

        // Extract salt and encrypted data
        if file_content.len() < 32 {
            anyhow::bail!("Invalid mnemonic file format");
        }

        let salt: [u8; 32] = file_content[0..32]
            .try_into()
            .context("Failed to extract salt")?;
        let encrypted_data = &file_content[32..];

        // Derive key from password
        let key = Self::derive_key(password, &salt);

        // Decrypt
        let decrypted_bytes = Self::xor_encrypt(encrypted_data, &key);

        // Convert back to string
        let mnemonic_str =
            String::from_utf8(decrypted_bytes).context("Failed to decode mnemonic")?;

        // Parse mnemonic
        let mnemonic =
            Mnemonic::from_str(&mnemonic_str).context("Failed to parse stored mnemonic")?;

        Ok(Some(mnemonic))
    }

    /// Check if mnemonic exists in storage
    pub async fn has_mnemonic(base_path: &Path) -> Result<bool> {
        let path = Self::get_mnemonic_path(base_path);
        Ok(path.exists())
    }

    /// Derive encryption key from password
    fn derive_key(password: Option<&str>, salt: &[u8; 32]) -> [u8; 32] {
        let password = password.unwrap_or("default");

        // Simple key derivation using SHA256
        let mut hasher = Sha256::new();
        hasher.update(password.as_bytes());
        hasher.update(salt);

        let result = hasher.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&result);

        key
    }

    /// Simple XOR encryption/decryption
    fn xor_encrypt(data: &[u8], key: &[u8; 32]) -> Vec<u8> {
        data.iter()
            .enumerate()
            .map(|(i, &byte)| byte ^ key[i % key.len()])
            .collect()
    }
}
