//! Secure mnemonic storage with AES-256-GCM encryption and Argon2 key derivation
//!
//! This module provides secure storage for BIP39 mnemonics using:
//! - AES-256-GCM for authenticated encryption
//! - Argon2id for password-based key derivation
//! - Random salts and nonces for each encryption operation
//!
//! File format: salt (16 bytes) + nonce (12 bytes) + ciphertext (variable length)

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{ensure, Context, Result};
use argon2::{
    password_hash::{PasswordHasher, SaltString},
    Argon2, ParamsBuilder, Version,
};
use fedimint_bip39::Mnemonic;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Salt size for Argon2 (16 bytes / 128 bits)
const SALT_SIZE: usize = 16;

/// Nonce size for AES-GCM (12 bytes / 96 bits)
const NONCE_SIZE: usize = 12;

/// Mnemonic storage operations with secure encryption
pub struct MnemonicStore;

impl MnemonicStore {
    /// Get the mnemonic file path
    fn get_mnemonic_path(base_path: &Path) -> PathBuf {
        base_path.join(".mnemonic")
    }

    /// Store a mnemonic with password encryption
    ///
    /// # Security
    /// - Uses AES-256-GCM for authenticated encryption
    /// - Uses Argon2id for password-based key derivation
    /// - Generates random salt and nonce for each operation
    /// - Requires a password (no default fallback)
    ///
    /// # Arguments
    /// - `base_path`: Directory where the mnemonic file will be stored
    /// - `mnemonic`: The BIP39 mnemonic to encrypt and store
    /// - `password`: Password for encryption (required, not optional despite type)
    pub async fn store_mnemonic(
        base_path: &Path,
        mnemonic: &Mnemonic,
        password: Option<&str>,
    ) -> Result<()> {
        let path = Self::get_mnemonic_path(base_path);

        // Require password - no default fallback for security
        let password = password.context("Password is required for mnemonic encryption")?;

        ensure!(!password.is_empty(), "Password cannot be empty");

        // Generate random salt for Argon2
        let salt_bytes: [u8; SALT_SIZE] = rand::random();

        // Generate random nonce for AES-GCM
        let nonce_bytes: [u8; NONCE_SIZE] = rand::random();
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Derive encryption key using Argon2id
        let key =
            Self::derive_key(password, &salt_bytes).context("Failed to derive encryption key")?;

        // Create AES-256-GCM cipher
        let cipher = Aes256Gcm::new_from_slice(&key).context("Failed to create AES cipher")?;

        // Convert mnemonic to bytes
        let mnemonic_str = mnemonic.to_string();
        let plaintext = mnemonic_str.as_bytes();

        // Encrypt with authenticated encryption (AES-256-GCM)
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|error| anyhow::anyhow!("Encryption failed: {error}"))?;

        // Combine: salt (16 bytes) + nonce (12 bytes) + ciphertext
        let mut file_content = Vec::with_capacity(SALT_SIZE + NONCE_SIZE + ciphertext.len());
        file_content.extend_from_slice(&salt_bytes);
        file_content.extend_from_slice(&nonce_bytes);
        file_content.extend_from_slice(&ciphertext);

        // Write to file
        tokio::fs::write(&path, file_content)
            .await
            .with_context(|| {
                format!(
                    "Failed to write mnemonic file to {path}",
                    path = path.display()
                )
            })?;

        Ok(())
    }

    /// Load mnemonic with password decryption
    ///
    /// # Security
    /// - Verifies authentication tag from AES-GCM
    /// - Returns error if password is incorrect or data is tampered
    ///
    /// # Arguments
    /// - `base_path`: Directory where the mnemonic file is stored
    /// - `password`: Password for decryption (required)
    ///
    /// # Returns
    /// - `Ok(Some(mnemonic))` if file exists and decryption succeeds
    /// - `Ok(None)` if file doesn't exist
    /// - `Err` if decryption fails or data is invalid
    pub async fn load_mnemonic(
        base_path: &Path,
        password: Option<&str>,
    ) -> Result<Option<Mnemonic>> {
        let path = Self::get_mnemonic_path(base_path);

        // Check if file exists
        if !path.exists() {
            return Ok(None);
        }

        // Require password
        let password = password.context("Password is required for mnemonic decryption")?;

        // Read file
        let file_content = tokio::fs::read(&path).await.with_context(|| {
            format!(
                "Failed to read mnemonic file from {path}",
                path = path.display()
            )
        })?;

        // Validate minimum file size: salt + nonce + at least some ciphertext
        let min_size = SALT_SIZE + NONCE_SIZE;
        ensure!(
            file_content.len() > min_size,
            "Invalid mnemonic file format: file too small (expected > {min_size} bytes, got {size} bytes)",
            min_size = min_size,
            size = file_content.len()
        );

        // Extract salt, nonce, and ciphertext
        let salt_bytes: [u8; SALT_SIZE] = file_content[0..SALT_SIZE]
            .try_into()
            .context("Failed to extract salt from mnemonic file")?;

        let nonce_bytes: [u8; NONCE_SIZE] = file_content[SALT_SIZE..SALT_SIZE + NONCE_SIZE]
            .try_into()
            .context("Failed to extract nonce from mnemonic file")?;

        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = &file_content[SALT_SIZE + NONCE_SIZE..];

        // Derive key from password using same salt
        let key =
            Self::derive_key(password, &salt_bytes).context("Failed to derive decryption key")?;

        // Create AES-256-GCM cipher
        let cipher = Aes256Gcm::new_from_slice(&key)
            .context("Failed to create AES cipher for decryption")?;

        // Decrypt and verify authentication tag
        let plaintext = cipher.decrypt(nonce, ciphertext).map_err(|error| {
            anyhow::anyhow!("Decryption failed (incorrect password or corrupted data): {error}")
        })?;

        // Convert back to string
        let mnemonic_str =
            String::from_utf8(plaintext).context("Failed to decode decrypted mnemonic as UTF-8")?;

        // Parse and validate mnemonic
        let mnemonic = Mnemonic::from_str(&mnemonic_str)
            .context("Failed to parse stored mnemonic (invalid BIP39 format)")?;

        Ok(Some(mnemonic))
    }

    /// Check if mnemonic exists in storage
    pub async fn has_mnemonic(base_path: &Path) -> Result<bool> {
        let path = Self::get_mnemonic_path(base_path);
        Ok(path.exists())
    }

    /// Derive encryption key from password using Argon2id
    ///
    /// Uses Argon2id with moderate parameters suitable for interactive use:
    /// - Memory cost: 64 MB (65536 KiB)
    /// - Time cost: 3 iterations
    /// - Parallelism: 4 lanes
    /// - Output: 32 bytes (256 bits) for AES-256
    ///
    /// # Arguments
    /// - `password`: User-provided password
    /// - `salt`: Random salt bytes (16 bytes minimum)
    ///
    /// # Returns
    /// 32-byte key suitable for AES-256
    fn derive_key(password: &str, salt: &[u8; SALT_SIZE]) -> Result<[u8; 32]> {
        // Configure Argon2id parameters
        // These are moderate parameters suitable for interactive use
        // Memory: 64 MB, Time: 3 iterations, Parallelism: 4
        let params = ParamsBuilder::new()
            .m_cost(65536) // 64 MB memory cost
            .t_cost(3) // 3 iterations
            .p_cost(4) // 4 parallel lanes
            .output_len(32) // 256-bit output for AES-256
            .build()
            .map_err(|error| anyhow::anyhow!("Failed to build Argon2 parameters: {error}"))?;

        let argon2 = Argon2::new(argon2::Algorithm::Argon2id, Version::V0x13, params);

        // Create salt string from bytes
        let salt_str = SaltString::encode_b64(salt)
            .map_err(|error| anyhow::anyhow!("Failed to encode salt for Argon2: {error}"))?;

        // Hash password to derive key
        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt_str)
            .map_err(|error| anyhow::anyhow!("Argon2 key derivation failed: {error}"))?;

        // Extract the hash bytes (key material)
        let hash_bytes = password_hash
            .hash
            .context("Argon2 hash output is missing")?;

        // Convert to 32-byte array
        let key_bytes = hash_bytes.as_bytes();
        ensure!(
            key_bytes.len() == 32,
            "Argon2 output length mismatch: expected 32 bytes, got {len} bytes",
            len = key_bytes.len()
        );

        let mut key = [0u8; 32];
        key.copy_from_slice(key_bytes);

        Ok(key)
    }
}
