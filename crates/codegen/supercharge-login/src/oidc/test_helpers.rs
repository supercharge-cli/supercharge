//! Test helpers shared by `oidc::protocol::tests` and `oidc::login::tests`: a mock IdP server and JWT signing primitives.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::protocol::{Discovery, discover};

pub(super) const TEST_KID: &str = "test-kid";
pub(super) fn test_nonce() -> String {
    format!("tn-{:x}", std::process::id())
}
pub(super) const TEST_CLIENT_ID: &str = "test-client-id";
pub(super) fn ensure_crypto_provider() {
    supercharge_extra_ca::ensure_default_crypto_provider();
    let _ = jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER.install_default();
}
fn take_der_tlv<'a>(input: &'a [u8], offset: &mut usize, tag: u8) -> &'a [u8] {
    assert_eq!(input.get(*offset), Some(&tag), "unexpected DER tag");
    *offset += 1;
    let first_len = *input.get(*offset).expect("missing DER length");
    *offset += 1;
    let length = if first_len & 0x80 == 0 {
        first_len as usize
    } else {
        let count = (first_len & 0x7f) as usize;
        assert!(count > 0 && count <= 4, "unsupported DER length");
        let mut length = 0usize;
        for _ in 0..count {
            length = (length << 8) | (*input.get(*offset).expect("truncated DER length") as usize);
            *offset += 1;
        }
        length
    };
    let end = offset.checked_add(length).expect("DER length overflow");
    let value = input.get(*offset..end).expect("truncated DER value");
    *offset = end;
    value
}

fn rsa_public_components(public_key_der: &[u8]) -> (Vec<u8>, Vec<u8>) {
    // SubjectPublicKeyInfo ::= SEQUENCE { algorithm, subjectPublicKey BIT STRING }
    let mut outer_offset = 0;
    let outer = take_der_tlv(public_key_der, &mut outer_offset, 0x30);
    let mut outer_contents_offset = 0;
    let _algorithm = take_der_tlv(outer, &mut outer_contents_offset, 0x30);
    let bit_string = take_der_tlv(outer, &mut outer_contents_offset, 0x03);
    assert_eq!(
        bit_string.first(),
        Some(&0),
        "RSA bit string must be byte-aligned"
    );

    // RSAPublicKey ::= SEQUENCE { modulus INTEGER, publicExponent INTEGER }
    let mut rsa_offset = 0;
    let rsa_public_key = take_der_tlv(&bit_string[1..], &mut rsa_offset, 0x30);
    let mut rsa_contents_offset = 0;
    let modulus = take_der_tlv(rsa_public_key, &mut rsa_contents_offset, 0x02);
    let exponent = take_der_tlv(rsa_public_key, &mut rsa_contents_offset, 0x02);
    let modulus = modulus.strip_prefix(&[0]).unwrap_or(modulus).to_vec();
    (modulus, exponent.to_vec())
}

pub(super) fn generate_test_rsa_key() -> (String, String, String) {
    let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_RSA_SHA256).unwrap();
    let (modulus, exponent) = rsa_public_components(&key_pair.public_key_der());
    let pem = key_pair.serialize_pem();
    let jwk_n = URL_SAFE_NO_PAD.encode(modulus);
    let jwk_e = URL_SAFE_NO_PAD.encode(exponent);
    (pem, jwk_n, jwk_e)
}
pub(super) async fn mock_idp_token() -> (String, String, Discovery, tokio::task::JoinHandle<()>) {
    let (issuer, handle) = start_mock_idp().await;
    let discovery = discover(&issuer).await.unwrap();
    let resp: serde_json::Value = supercharge_http::shared_client()
        .post(&discovery.token_endpoint)
        .form(&[("grant_type", "authorization_code")])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id_token = resp
        .get("id_token")
        .and_then(|v| v.as_str())
        .expect("mock missing id_token")
        .to_string();
    (issuer, id_token, discovery, handle)
}
pub(super) async fn start_mock_idp() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let issuer_for_discovery = issuer.clone();
    let (rsa_pem, jwk_n, jwk_e) = generate_test_rsa_key();

    #[derive(serde::Serialize)]
    struct Claims {
        sub: &'static str,
        email: &'static str,
        iss: String,
        aud: &'static str,
        nonce: String,
        exp: usize,
    }

    let id_token = {
        let mut hdr = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        hdr.kid = Some(TEST_KID.to_owned());
        jsonwebtoken::encode(
            &hdr,
            &Claims {
                sub: "user-42",
                email: "test@corp.com",
                iss: issuer.clone(),
                aud: TEST_CLIENT_ID,
                nonce: test_nonce(),
                exp: (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize,
            },
            &jsonwebtoken::EncodingKey::from_rsa_pem(rsa_pem.as_bytes()).unwrap(),
        )
        .unwrap()
    };

    let app = axum::Router::new()
        .route(
            "/.well-known/openid-configuration",
            axum::routing::get(move || {
                let iss = issuer_for_discovery.clone();
                async move {
                    axum::Json(serde_json::json!({
                        "authorization_endpoint": format!("{iss}/authorize"),
                        "token_endpoint": format!("{iss}/token"),
                        "jwks_uri": format!("{iss}/jwks"),
                        "id_token_signing_alg_values_supported": ["RS256"],
                    }))
                }
            }),
        )
        .route(
            "/jwks",
            axum::routing::get(move || {
                let n = jwk_n.clone();
                let e = jwk_e.clone();
                async move {
                    axum::Json(serde_json::json!({
                        "keys": [{
                            "kty": "RSA", "alg": "RS256", "kid": TEST_KID,
                            "n": n, "e": e,
                        }]
                    }))
                }
            }),
        )
        .route(
            "/token",
            axum::routing::post(move || {
                let tok = id_token.clone();
                async move {
                    axum::Json(serde_json::json!({
                        "access_token": "mock-access-token",
                        "refresh_token": "mock-refresh-token",
                        "id_token": tok,
                        "expires_in": 3600,
                    }))
                }
            }),
        );

    let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (issuer, handle)
}
