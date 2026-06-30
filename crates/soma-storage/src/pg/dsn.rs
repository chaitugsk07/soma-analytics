//! DSN encrypt/decrypt helpers.
//!
//! Encrypts a DSN string with `soma_infra::crypto::encrypt(kek, dsn, aad=tenant_id_bytes)`.
//! NULL `dsn_ciphertext` in 02_fct_data_sources means "use the env ANALYTICS_DB_URL pool"
//! (Phase-1 default). Non-NULL means a per-source encrypted DSN for multi-source (Phase 1.5+).

use soma_infra::crypto::{decrypt, encrypt, CryptoKey, CryptoError};
use uuid::Uuid;

/// Encrypt a plaintext DSN string for a given tenant.
///
/// The `tenant_id` bytes are used as the AAD so that a ciphertext produced for
/// one tenant cannot be decrypted under another tenant's context.
pub fn encrypt_dsn(kek: &CryptoKey, tenant_id: Uuid, dsn: &str) -> Result<Vec<u8>, CryptoError> {
    let aad = tenant_id.as_bytes();
    encrypt(kek, dsn.as_bytes(), aad)
}

/// Decrypt a DSN ciphertext for a given tenant.
pub fn decrypt_dsn(
    kek: &CryptoKey,
    tenant_id: Uuid,
    ciphertext: &[u8],
) -> Result<String, CryptoError> {
    let aad = tenant_id.as_bytes();
    let plaintext = decrypt(kek, ciphertext, aad)?;
    String::from_utf8(plaintext).map_err(|_| CryptoError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soma_infra::crypto::CryptoKey;
    use uuid::Uuid;

    fn test_key() -> CryptoKey {
        CryptoKey::from_bytes(&[0x42u8; 32]).unwrap()
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = test_key();
        let tenant = Uuid::new_v4();
        let dsn = "postgres://readonly:secret@db:5432/soma?sslmode=require";
        let ct = encrypt_dsn(&key, tenant, dsn).unwrap();
        let recovered = decrypt_dsn(&key, tenant, &ct).unwrap();
        assert_eq!(recovered, dsn);
    }

    #[test]
    fn wrong_tenant_fails_decrypt() {
        let key = test_key();
        let tenant1 = Uuid::new_v4();
        let tenant2 = Uuid::new_v4();
        let ct = encrypt_dsn(&key, tenant1, "postgres://x").unwrap();
        assert!(
            decrypt_dsn(&key, tenant2, &ct).is_err(),
            "different tenant AAD must fail authentication"
        );
    }

    #[test]
    fn null_dsn_convention() {
        // Phase-1 default: dsn_ciphertext IS NULL in the DB → use ANALYTICS_DB_URL env pool.
        // This test documents the convention; no crypto involved.
        let ciphertext: Option<Vec<u8>> = None;
        assert!(ciphertext.is_none(), "NULL dsn_ciphertext → use env pool");
    }
}
