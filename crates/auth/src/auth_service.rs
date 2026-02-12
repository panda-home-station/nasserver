use async_trait::async_trait;
use sqlx::{Pool, Sqlite};
use uuid::Uuid;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use password_hash::SaltString;
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use std::path::Path;
use crate::AuthService;
use domain::{Result, Error as DomainError, auth::{SignupReq, SignupResp, LoginReq, LoginResp, Claims}};
// Remove models import

pub struct AuthServiceImpl {
    db: Pool<Sqlite>,
    jwt_secret: String,
    storage_path: String,
}

impl AuthServiceImpl {
    pub fn new(db: Pool<Sqlite>, jwt_secret: String, storage_path: String) -> Self {
        Self { db, jwt_secret, storage_path }
    }
}

#[async_trait]
impl AuthService for AuthServiceImpl {
    async fn signup(&self, req: SignupReq) -> Result<SignupResp> {
        let existing = sqlx::query_scalar::<_, Uuid>("select id from users where username = $1")
            .bind(&req.username)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
            
        if let Some(uid) = existing {
            return Ok(SignupResp { user_id: uid.to_string() });
        }

        let salt = SaltString::generate(&mut rand_core::OsRng);
        let hash = Argon2::default()
            .hash_password(req.password.as_bytes(), &salt)
            .map_err(|e| DomainError::Internal(e.to_string()))?
            .to_string();
            
        let uid = Uuid::new_v4();
        sqlx::query("insert into users (id, username, password_hash) values ($1, $2, $3)")
            .bind(uid)
            .bind(&req.username)
            .bind(&hash)
            .execute(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
            
        // create per-user storage root
        let user_root = Path::new(&self.storage_path).join("vol1").join("User").join(&req.username);
        let _ = std::fs::create_dir_all(&user_root);
        
        Ok(SignupResp { user_id: uid.to_string() })
    }

    async fn login(&self, req: LoginReq) -> Result<LoginResp> {
        let row = sqlx::query_as::<_, (Uuid, String, String)>("select id, password_hash, username from users where username = $1")
            .bind(&req.username)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
            
        if let Some((uid, pwd_hash, username)) = row {
            let parsed = PasswordHash::new(&pwd_hash)
                .map_err(|e| DomainError::Internal(e.to_string()))?;
                
            let ok = Argon2::default()
                .verify_password(req.password.as_bytes(), &parsed)
                .is_ok();
                
            if ok {
                let exp = (Utc::now() + Duration::days(7)).timestamp() as usize;
                let claims = Claims {
                    sub: uid.to_string(),
                    name: username,
                    exp,
                };
                let token = encode(
                    &Header::default(),
                    &claims,
                    &EncodingKey::from_secret(self.jwt_secret.as_bytes()),
                ).map_err(|e| DomainError::Internal(e.to_string()))?;
                
                return Ok(LoginResp {
                    user_id: uid.to_string(),
                    token,
                });
            }
        }
        
        Err(DomainError::Unauthorized("Invalid credentials".to_string()))
    }

    async fn get_user_by_id(&self, user_id: &str) -> Result<serde_json::Value> {
        // Try to parse as Uuid to ensure format matching if DB uses Uuid type
        let uid = Uuid::parse_str(user_id).map_err(|e| DomainError::BadRequest(format!("Invalid UUID: {}", e)))?;
        
        let username = sqlx::query_scalar::<_, String>("select username from users where id = $1")
            .bind(uid) // Bind as Uuid, not &str
            .fetch_optional(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
            
        if let Some(name) = username {
            Ok(serde_json::json!({
                "id": user_id,
                "username": name,
            }))
        } else {
            Err(DomainError::NotFound("User not found".to_string()))
        }
    }

    async fn get_wallpaper(&self, user_id: &str) -> Result<Option<String>> {
        let wallpaper = sqlx::query_scalar::<_, Option<String>>("select wallpaper from users where id = $1")
            .bind(user_id)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?
            .flatten();
            
        Ok(wallpaper)
    }

    async fn set_wallpaper(&self, user_id: &str, path: &str) -> Result<()> {
        sqlx::query("update users set wallpaper = $1 where id = $2")
            .bind(path)
            .bind(user_id)
            .execute(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
            
        Ok(())
    }
}
