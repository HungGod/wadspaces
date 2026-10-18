//! Signing in with Firebase Auth's REST API: a custom token (a machine's, from
//! enrollMachine) or a GitHub access token (a person's, from the sign-in
//! page) for an ID token and a refresh token, and refreshing.

use serde_json::{Value, json};

use crate::Error;

const IDENTITY: &str = "https://identitytoolkit.googleapis.com/v1";
const SECURETOKEN: &str = "https://securetoken.googleapis.com/v1";

#[derive(Debug, Clone)]
pub struct Identity {
    http: reqwest::Client,
    identity: String,
    securetoken: String,
}

/// Who signed in with an identity provider (GitHub), as Firebase has them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdpSignIn {
    pub tokens: Tokens,
    /// The Firebase uid.
    pub uid: String,
    /// The provider's own id for them (GitHub's numeric user id).
    pub provider_uid: String,
    /// Their login there (GitHub's).
    pub screen_name: Option<String>,
    pub display_name: Option<String>,
    pub photo_url: Option<String>,
    pub is_new_user: bool,
}

/// An ID token, how long it lasts, and the refresh token to get the next one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub id_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

impl Identity {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http, identity: IDENTITY.into(), securetoken: SECURETOKEN.into() }
    }

    /// Against the Auth emulator (`127.0.0.1:9099`).
    pub fn emulator(http: reqwest::Client, host: &str) -> Self {
        Self {
            http,
            identity: format!("http://{host}/identitytoolkit.googleapis.com/v1"),
            securetoken: format!("http://{host}/securetoken.googleapis.com/v1"),
        }
    }

    /// Against a test server (both APIs under it).
    pub fn with_base(http: reqwest::Client, base: &str) -> Self {
        let b = base.trim_end_matches('/');
        Self { http, identity: format!("{b}/identitytoolkit"), securetoken: format!("{b}/securetoken") }
    }

    async fn answer(res: reqwest::Response) -> Result<Value, Error> {
        let status = res.status().as_u16();
        let body: Value = res.json().await.unwrap_or(Value::Null);
        if status == 200 {
            return Ok(body);
        }
        let message = body.pointer("/error/message").and_then(Value::as_str).unwrap_or("no reason given").to_string();
        Err(Error::Auth { status, message })
    }

    pub async fn sign_in_with_custom_token(&self, api_key: &str, token: &str) -> Result<Tokens, Error> {
        let res = self
            .http
            .post(format!("{}/accounts:signInWithCustomToken", self.identity))
            .query(&[("key", api_key)])
            .json(&json!({"token": token, "returnSecureToken": true}))
            .send()
            .await
            .map_err(Error::Network)?;
        let b = Self::answer(res).await?;
        Ok(Tokens {
            id_token: b["idToken"].as_str().unwrap_or_default().into(),
            refresh_token: b["refreshToken"].as_str().unwrap_or_default().into(),
            expires_in: b["expiresIn"].as_str().and_then(|s| s.parse().ok()).unwrap_or(3600),
        })
    }

    /// Signs in (or up) with a provider's access token: `provider` is its
    /// Firebase id ("github.com"). Firebase's own words for a refusal
    /// (ADMIN_ONLY_OPERATION: sign-ups closed) come back in Error::Auth.
    pub async fn sign_in_with_idp(
        &self,
        api_key: &str,
        provider: &str,
        access_token: &str,
    ) -> Result<IdpSignIn, Error> {
        self.idp(api_key, &format!("access_token={}&providerId={}", encode(access_token), encode(provider))).await
    }

    /// The Auth emulator's stand-in for a provider's sign-in (it can't do
    /// GitHub's): an unsigned ID token naming the person.
    pub async fn sign_in_with_fake_idp(
        &self,
        api_key: &str,
        provider: &str,
        claims: &Value,
    ) -> Result<IdpSignIn, Error> {
        self.idp(api_key, &format!("id_token={}&providerId={}", encode(&claims.to_string()), encode(provider))).await
    }

    async fn idp(&self, api_key: &str, post_body: &str) -> Result<IdpSignIn, Error> {
        let res = self
            .http
            .post(format!("{}/accounts:signInWithIdp", self.identity))
            .query(&[("key", api_key)])
            .json(&json!({
                "postBody": post_body,
                "requestUri": "http://localhost",
                "returnIdpCredential": true,
                "returnSecureToken": true,
            }))
            .send()
            .await
            .map_err(Error::Network)?;
        let b = Self::answer(res).await?;
        let text = |k: &str| b.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
        // federatedId is the provider's profile address ending in its user id;
        // rawUserInfo has the id itself.
        let raw: Value = text("rawUserInfo").and_then(|r| serde_json::from_str(&r).ok()).unwrap_or(Value::Null);
        let provider_uid = match raw.get("id") {
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::String(s)) => s.clone(),
            _ => text("federatedId").and_then(|f| f.rsplit('/').next().map(String::from)).unwrap_or_default(),
        };
        Ok(IdpSignIn {
            tokens: Tokens {
                id_token: text("idToken").unwrap_or_default(),
                refresh_token: text("refreshToken").unwrap_or_default(),
                expires_in: text("expiresIn").and_then(|s| s.parse().ok()).unwrap_or(3600),
            },
            uid: text("localId").unwrap_or_default(),
            provider_uid,
            screen_name: text("screenName").or_else(|| raw.get("login").and_then(Value::as_str).map(String::from)),
            display_name: text("displayName"),
            photo_url: text("photoUrl"),
            is_new_user: b.get("isNewUser").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// Whose an ID token is: its uid, if Firebase says the token is good
    /// (signed, not expired, the account not disabled). A token that isn't
    /// is Error::Auth (INVALID_ID_TOKEN, TOKEN_EXPIRED, USER_NOT_FOUND).
    pub async fn lookup(&self, api_key: &str, id_token: &str) -> Result<String, Error> {
        let res = self
            .http
            .post(format!("{}/accounts:lookup", self.identity))
            .query(&[("key", api_key)])
            .json(&json!({"idToken": id_token}))
            .send()
            .await
            .map_err(Error::Network)?;
        let b = Self::answer(res).await?;
        match b.pointer("/users/0/localId").and_then(Value::as_str) {
            Some(uid) if !uid.is_empty() => Ok(uid.to_string()),
            _ => Err(Error::Auth { status: 400, message: "USER_NOT_FOUND".into() }),
        }
    }

    pub async fn refresh(&self, api_key: &str, refresh_token: &str) -> Result<Tokens, Error> {
        let res = self
            .http
            .post(format!("{}/token", self.securetoken))
            .query(&[("key", api_key)])
            .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token)])
            .send()
            .await
            .map_err(Error::Network)?;
        let b = Self::answer(res).await?;
        Ok(Tokens {
            id_token: b["id_token"].as_str().unwrap_or_default().into(),
            refresh_token: b["refresh_token"].as_str().unwrap_or(refresh_token).into(),
            expires_in: b["expires_in"].as_str().and_then(|s| s.parse().ok()).unwrap_or(3600),
        })
    }
}

/// Form-encodes a value (what signInWithIdp's postBody holds).
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn signs_in_with_github() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/identitytoolkit/accounts:signInWithIdp"))
            .and(query_param("key", "k"))
            .and(body_partial_json(json!({"postBody": "access_token=gho_a%2Bb&providerId=github.com", "returnSecureToken": true})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "localId": "uid1", "idToken": "id", "refreshToken": "rt", "expiresIn": "3600",
                "federatedId": "https://github.com/4242", "screenName": "octo", "displayName": "Octo Cat",
                "photoUrl": "https://avatars.githubusercontent.com/u/4242", "rawUserInfo": "{\"id\":4242,\"login\":\"octo\"}",
                "isNewUser": true
            })))
            .mount(&s)
            .await;
        let got = Identity::with_base(reqwest::Client::new(), &s.uri())
            .sign_in_with_idp("k", "github.com", "gho_a+b")
            .await
            .unwrap();
        assert_eq!(
            (got.uid.as_str(), got.provider_uid.as_str(), got.screen_name.as_deref()),
            ("uid1", "4242", Some("octo"))
        );
        assert_eq!((got.tokens.refresh_token.as_str(), got.is_new_user), ("rt", true));
    }

    #[tokio::test]
    async fn a_refusal_says_why() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(400).set_body_json(json!({"error": {"message": "ADMIN_ONLY_OPERATION"}})),
            )
            .mount(&s)
            .await;
        let e = Identity::with_base(reqwest::Client::new(), &s.uri())
            .sign_in_with_idp("k", "github.com", "t")
            .await
            .unwrap_err();
        assert!(matches!(e, Error::Auth { status: 400, ref message } if message == "ADMIN_ONLY_OPERATION"), "{e:?}");
    }
}
