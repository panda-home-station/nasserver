use async_trait::async_trait;
use crate::Result;
pub use crate::entities::auth::{Claims, AuthUser};
pub use crate::dtos::auth::{SignupReq, SignupResp, LoginReq, LoginResp, SecuritySettings};

#[async_trait]
pub trait AuthService: Send + Sync {
    async fn signup(&self, req: SignupReq) -> Result<SignupResp>;
    async fn login(&self, req: LoginReq) -> Result<LoginResp>;
    async fn get_user_by_id(&self, user_id: &str) -> Result<serde_json::Value>;
    async fn get_wallpaper(&self, user_id: &str) -> Result<Option<String>>;
    async fn set_wallpaper(&self, user_id: &str, path: &str) -> Result<()>;
    async fn get_security_settings(&self, user_id: &str) -> Result<SecuritySettings>;
    async fn set_security_settings(&self, user_id: &str, settings: SecuritySettings) -> Result<()>;
}
