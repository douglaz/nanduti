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
use hkdf::Hkdf;
use sha2::Sha256;
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

        // Write to file with owner-only permissions (0600) to prevent other
        // local users from reading the encrypted mnemonic for offline brute-force.
        {
            use std::io::Write;
            #[cfg(unix)]
            use std::os::unix::fs::OpenOptionsExt;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            opts.mode(0o600);
            let mut file = opts
                .open(&path)
                .with_context(|| format!("Failed to create mnemonic file: {}", path.display()))?;
            file.write_all(&file_content).with_context(|| {
                format!(
                    "Failed to write mnemonic file to {path}",
                    path = path.display()
                )
            })?;
        }

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

    /// Derive a storage encryption key from a mnemonic
    ///
    /// Uses HKDF-SHA256 to derive a 256-bit key from the mnemonic's entropy.
    /// This key is deterministic - the same mnemonic always produces the same key.
    ///
    /// # Security
    /// - Uses HKDF (RFC 5869) for secure key derivation
    /// - Info string "nanduti-storage-v1" provides domain separation
    /// - No salt needed since mnemonic entropy is already high-entropy
    ///
    /// # Arguments
    /// - `mnemonic`: The BIP39 mnemonic to derive the key from
    ///
    /// # Returns
    /// 32-byte key suitable for AES-256-GCM encryption
    pub fn derive_storage_key(mnemonic: &Mnemonic) -> Result<[u8; 32]> {
        Self::derive_key_with_info(mnemonic, b"nanduti-storage-v1")
    }

    /// Derive a Nostr wallet secret key from a mnemonic
    ///
    /// Uses HKDF-SHA256 with a distinct info string to derive a 32-byte key
    /// suitable for use as a Nostr/secp256k1 secret key. This ensures the
    /// wallet identity is deterministic and persists across restarts.
    ///
    /// # Arguments
    /// - `mnemonic`: The BIP39 mnemonic to derive the key from
    ///
    /// # Returns
    /// 32-byte key as a hex string, suitable for `NwcKeys::from_secret()`
    pub fn derive_nostr_wallet_key(mnemonic: &Mnemonic) -> Result<String> {
        let key = Self::derive_key_with_info(mnemonic, b"nanduti-nostr-wallet-v1")?;
        Ok(hex::encode(key))
    }

    /// Internal helper: derive a 32-byte key from a mnemonic with the given HKDF info string
    fn derive_key_with_info(mnemonic: &Mnemonic, info: &[u8]) -> Result<[u8; 32]> {
        // Get mnemonic as input key material
        // Using the mnemonic string as IKM (it contains ~128-256 bits of entropy)
        let ikm = mnemonic.to_string();

        // Create HKDF instance with SHA-256
        // No salt needed - mnemonic already has sufficient entropy
        let hk = Hkdf::<Sha256>::new(None, ikm.as_bytes());

        // Derive key with domain-specific info string for separation
        let mut key = [0u8; 32];
        hk.expand(info, &mut key)
            .map_err(|_| anyhow::anyhow!("HKDF key expansion failed"))?;

        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_mnemonic_encryption_round_trip() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let password = "test-password-12345";

        // Generate a test mnemonic
        let entropy = [0u8; 16];
        let original_mnemonic = Mnemonic::from_entropy(&entropy)?;

        // Store the mnemonic
        MnemonicStore::store_mnemonic(temp_dir.path(), &original_mnemonic, Some(password)).await?;

        // Verify file exists
        assert!(MnemonicStore::has_mnemonic(temp_dir.path()).await?);

        // Load and verify
        let loaded_mnemonic = MnemonicStore::load_mnemonic(temp_dir.path(), Some(password)).await?;

        assert!(loaded_mnemonic.is_some());
        assert_eq!(
            original_mnemonic.to_string(),
            loaded_mnemonic.unwrap().to_string()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_wrong_password_fails() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let correct_password = "correct-password";
        let wrong_password = "wrong-password";

        // Generate and store mnemonic
        let entropy = [0u8; 16];
        let mnemonic = Mnemonic::from_entropy(&entropy)?;

        MnemonicStore::store_mnemonic(temp_dir.path(), &mnemonic, Some(correct_password)).await?;

        // Try to load with wrong password - should fail
        let result = MnemonicStore::load_mnemonic(temp_dir.path(), Some(wrong_password)).await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Decryption failed"));

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_no_password_store_fails() -> Result<()> {
        let temp_dir = TempDir::new()?;

        // Generate mnemonic
        let entropy = [0u8; 16];
        let mnemonic = Mnemonic::from_entropy(&entropy)?;

        // Try to store without password - should fail
        let result = MnemonicStore::store_mnemonic(temp_dir.path(), &mnemonic, None).await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Password is required"));

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_empty_password_fails() -> Result<()> {
        let temp_dir = TempDir::new()?;

        // Generate mnemonic
        let entropy = [0u8; 16];
        let mnemonic = Mnemonic::from_entropy(&entropy)?;

        // Try to store with empty password - should fail
        let result = MnemonicStore::store_mnemonic(temp_dir.path(), &mnemonic, Some("")).await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Password cannot be empty"));

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_load_nonexistent_returns_none() -> Result<()> {
        let temp_dir = TempDir::new()?;

        // Try to load from empty directory
        let result = MnemonicStore::load_mnemonic(temp_dir.path(), Some("password")).await?;

        assert!(result.is_none());
        assert!(!MnemonicStore::has_mnemonic(temp_dir.path()).await?);

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_different_passwords_produce_different_ciphertexts() -> Result<()> {
        let temp_dir1 = TempDir::new()?;
        let temp_dir2 = TempDir::new()?;

        // Same mnemonic, different passwords
        let entropy = [0u8; 16];
        let mnemonic = Mnemonic::from_entropy(&entropy)?;

        MnemonicStore::store_mnemonic(temp_dir1.path(), &mnemonic, Some("password1")).await?;
        MnemonicStore::store_mnemonic(temp_dir2.path(), &mnemonic, Some("password2")).await?;

        // Read the encrypted files
        let file1 = tokio::fs::read(temp_dir1.path().join(".mnemonic")).await?;
        let file2 = tokio::fs::read(temp_dir2.path().join(".mnemonic")).await?;

        // Ciphertexts should be different (due to different salts/nonces/keys)
        assert_ne!(file1, file2);

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_tampered_data_fails() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let password = "test-password";

        // Generate and store mnemonic
        let entropy = [0u8; 16];
        let mnemonic = Mnemonic::from_entropy(&entropy)?;
        MnemonicStore::store_mnemonic(temp_dir.path(), &mnemonic, Some(password)).await?;

        // Read the file
        let mnemonic_path = temp_dir.path().join(".mnemonic");
        let mut file_content = tokio::fs::read(&mnemonic_path).await?;

        // Tamper with the ciphertext (flip a bit in the last byte)
        let last_idx = file_content.len() - 1;
        file_content[last_idx] ^= 0x01;

        // Write back tampered data
        tokio::fs::write(&mnemonic_path, file_content).await?;

        // Try to load - should fail authentication
        let result = MnemonicStore::load_mnemonic(temp_dir.path(), Some(password)).await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Decryption failed"));

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_multiple_store_load_cycles() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let password = "test-password";

        // First cycle
        let entropy1 = [1u8; 16];
        let mnemonic1 = Mnemonic::from_entropy(&entropy1)?;
        MnemonicStore::store_mnemonic(temp_dir.path(), &mnemonic1, Some(password)).await?;

        let loaded1 = MnemonicStore::load_mnemonic(temp_dir.path(), Some(password))
            .await?
            .unwrap();
        assert_eq!(mnemonic1.to_string(), loaded1.to_string());

        // Second cycle (overwrite)
        let entropy2 = [2u8; 16];
        let mnemonic2 = Mnemonic::from_entropy(&entropy2)?;
        MnemonicStore::store_mnemonic(temp_dir.path(), &mnemonic2, Some(password)).await?;

        let loaded2 = MnemonicStore::load_mnemonic(temp_dir.path(), Some(password))
            .await?
            .unwrap();
        assert_eq!(mnemonic2.to_string(), loaded2.to_string());

        // Should have the second mnemonic, not the first
        assert_ne!(mnemonic1.to_string(), loaded2.to_string());

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_concurrent_store_operations() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let password = "test-password";

        // Generate test mnemonics
        let entropy1 = [1u8; 16];
        let mnemonic1 = Mnemonic::from_entropy(&entropy1)?;
        let entropy2 = [2u8; 16];
        let mnemonic2 = Mnemonic::from_entropy(&entropy2)?;

        // Clone Arc for concurrent access
        let path1 = temp_dir.path().to_path_buf();
        let path2 = temp_dir.path().to_path_buf();

        // Store concurrently from multiple tasks
        let handle1 = tokio::spawn(async move {
            MnemonicStore::store_mnemonic(&path1, &mnemonic1, Some(password)).await
        });

        let handle2 = tokio::spawn(async move {
            MnemonicStore::store_mnemonic(&path2, &mnemonic2, Some(password)).await
        });

        // Wait for both to complete - one should succeed
        let result1 = handle1.await?;
        let result2 = handle2.await?;

        // At least one should succeed (last write wins)
        assert!(result1.is_ok() || result2.is_ok());

        // Verify we can load the mnemonic that was written last
        let loaded = MnemonicStore::load_mnemonic(temp_dir.path(), Some(password)).await?;
        assert!(loaded.is_some());

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_concurrent_load_operations() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let password = "test-password";

        // Store a mnemonic first
        let entropy = [0u8; 16];
        let mnemonic = Mnemonic::from_entropy(&entropy)?;
        MnemonicStore::store_mnemonic(temp_dir.path(), &mnemonic, Some(password)).await?;

        // Load concurrently from multiple tasks
        let path1 = temp_dir.path().to_path_buf();
        let path2 = temp_dir.path().to_path_buf();
        let path3 = temp_dir.path().to_path_buf();

        let handle1 =
            tokio::spawn(async move { MnemonicStore::load_mnemonic(&path1, Some(password)).await });

        let handle2 =
            tokio::spawn(async move { MnemonicStore::load_mnemonic(&path2, Some(password)).await });

        let handle3 =
            tokio::spawn(async move { MnemonicStore::load_mnemonic(&path3, Some(password)).await });

        // All should succeed with the same mnemonic
        let result1 = handle1.await??;
        let result2 = handle2.await??;
        let result3 = handle3.await??;

        assert!(result1.is_some());
        assert!(result2.is_some());
        assert!(result3.is_some());

        assert_eq!(result1.unwrap().to_string(), mnemonic.to_string());
        assert_eq!(result2.unwrap().to_string(), mnemonic.to_string());
        assert_eq!(result3.unwrap().to_string(), mnemonic.to_string());

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_truncated_file_fails() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let password = "test-password";

        // Generate and store mnemonic
        let entropy = [0u8; 16];
        let mnemonic = Mnemonic::from_entropy(&entropy)?;
        MnemonicStore::store_mnemonic(temp_dir.path(), &mnemonic, Some(password)).await?;

        // Read the file
        let mnemonic_path = temp_dir.path().join(".mnemonic");
        let file_content = tokio::fs::read(&mnemonic_path).await?;

        // Test various truncation scenarios
        let truncation_points = vec![
            0,                          // Empty file
            SALT_SIZE - 1,              // Incomplete salt
            SALT_SIZE,                  // Only salt
            SALT_SIZE + NONCE_SIZE - 1, // Incomplete nonce
            SALT_SIZE + NONCE_SIZE,     // Salt + nonce, no ciphertext
        ];

        for truncate_at in truncation_points {
            let truncated = &file_content[0..truncate_at];
            tokio::fs::write(&mnemonic_path, truncated).await?;

            let result = MnemonicStore::load_mnemonic(temp_dir.path(), Some(password)).await;
            assert!(
                result.is_err(),
                "Truncated file at {truncate_at} bytes should fail"
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_corrupted_salt_fails() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let password = "test-password";

        // Generate and store mnemonic
        let entropy = [0u8; 16];
        let mnemonic = Mnemonic::from_entropy(&entropy)?;
        MnemonicStore::store_mnemonic(temp_dir.path(), &mnemonic, Some(password)).await?;

        // Read the file
        let mnemonic_path = temp_dir.path().join(".mnemonic");
        let mut file_content = tokio::fs::read(&mnemonic_path).await?;

        // Corrupt the salt (first SALT_SIZE bytes)
        file_content[0] ^= 0xFF;
        file_content[SALT_SIZE - 1] ^= 0xFF;

        // Write back corrupted data
        tokio::fs::write(&mnemonic_path, file_content).await?;

        // Try to load - should fail (different salt = different key = decryption failure)
        let result = MnemonicStore::load_mnemonic(temp_dir.path(), Some(password)).await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Decryption failed"));

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_corrupted_nonce_fails() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let password = "test-password";

        // Generate and store mnemonic
        let entropy = [0u8; 16];
        let mnemonic = Mnemonic::from_entropy(&entropy)?;
        MnemonicStore::store_mnemonic(temp_dir.path(), &mnemonic, Some(password)).await?;

        // Read the file
        let mnemonic_path = temp_dir.path().join(".mnemonic");
        let mut file_content = tokio::fs::read(&mnemonic_path).await?;

        // Corrupt the nonce (SALT_SIZE to SALT_SIZE + NONCE_SIZE)
        file_content[SALT_SIZE] ^= 0xFF;
        file_content[SALT_SIZE + NONCE_SIZE - 1] ^= 0xFF;

        // Write back corrupted data
        tokio::fs::write(&mnemonic_path, file_content).await?;

        // Try to load - should fail (different nonce = decryption failure)
        let result = MnemonicStore::load_mnemonic(temp_dir.path(), Some(password)).await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Decryption failed"));

        Ok(())
    }

    #[tokio::test]
    async fn test_mnemonic_invalid_utf8_in_ciphertext() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let password = "test-password";

        // Create a mnemonic file with invalid UTF-8 after decryption
        // We'll manually construct a file that will decrypt to invalid UTF-8
        let salt_bytes: [u8; SALT_SIZE] = rand::random();
        let nonce_bytes: [u8; NONCE_SIZE] = rand::random();

        // Derive key
        let key = MnemonicStore::derive_key(password, &salt_bytes)?;
        let cipher = Aes256Gcm::new_from_slice(&key)?;
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Create invalid UTF-8 bytes (0xFF is not valid UTF-8)
        let invalid_utf8 = vec![0xFF, 0xFE, 0xFD];

        // Encrypt the invalid UTF-8
        let ciphertext = cipher
            .encrypt(nonce, invalid_utf8.as_slice())
            .map_err(|e| anyhow::anyhow!("Encryption failed: {e}"))?;

        // Construct file content
        let mut file_content = Vec::new();
        file_content.extend_from_slice(&salt_bytes);
        file_content.extend_from_slice(&nonce_bytes);
        file_content.extend_from_slice(&ciphertext);

        // Write the file
        let mnemonic_path = temp_dir.path().join(".mnemonic");
        tokio::fs::write(&mnemonic_path, file_content).await?;

        // Try to load - should fail with UTF-8 error
        let result = MnemonicStore::load_mnemonic(temp_dir.path(), Some(password)).await;

        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("Failed to decode") || error_msg.contains("UTF-8"),
            "Error should mention UTF-8 decoding failure, got: {error_msg}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_derive_storage_key_deterministic() -> Result<()> {
        // Same mnemonic should always produce same key
        let entropy = [42u8; 16];
        let mnemonic = Mnemonic::from_entropy(&entropy)?;

        let key1 = MnemonicStore::derive_storage_key(&mnemonic)?;
        let key2 = MnemonicStore::derive_storage_key(&mnemonic)?;

        assert_eq!(key1, key2);
        Ok(())
    }

    #[tokio::test]
    async fn test_derive_storage_key_different_mnemonics() -> Result<()> {
        // Different mnemonics should produce different keys
        let entropy1 = [1u8; 16];
        let entropy2 = [2u8; 16];

        let mnemonic1 = Mnemonic::from_entropy(&entropy1)?;
        let mnemonic2 = Mnemonic::from_entropy(&entropy2)?;

        let key1 = MnemonicStore::derive_storage_key(&mnemonic1)?;
        let key2 = MnemonicStore::derive_storage_key(&mnemonic2)?;

        assert_ne!(key1, key2);
        Ok(())
    }

    #[tokio::test]
    async fn test_derive_storage_key_length() -> Result<()> {
        // Key should be exactly 32 bytes for AES-256
        let entropy = [0u8; 16];
        let mnemonic = Mnemonic::from_entropy(&entropy)?;

        let key = MnemonicStore::derive_storage_key(&mnemonic)?;
        assert_eq!(key.len(), 32);
        Ok(())
    }
}
