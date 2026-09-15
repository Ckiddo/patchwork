//! Only hashes cross this boundary. No raw access or recovery credentials are stored.
use super::{Database, StoreError as E, transaction::TxError};
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, Row, postgres::PgRow};
use uuid::Uuid;

#[derive(Clone)]
pub struct Profile {
    pub id: Uuid,
    pub nickname: String,
    pub created_at: i64,
    pub auth_version: i64,
}
fn profile(row: &PgRow) -> Profile {
    Profile {
        id: row.get("user_id"),
        nickname: row.get("nickname"),
        created_at: row.get::<DateTime<Utc>, _>("created_at").timestamp(),
        auth_version: row.get("auth_version"),
    }
}
async fn locked_user(c: &mut PgConnection, id: Uuid) -> Result<PgRow, TxError> {
    sqlx::query("SELECT * FROM patchwork.users WHERE user_id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(c)
        .await?
        .ok_or(E::Permission.into())
}
async fn active_session(c: &mut PgConnection, user: Uuid, sid: Uuid) -> Result<PgRow, TxError> {
    sqlx::query("SELECT * FROM patchwork.sessions WHERE user_id=$1 AND session_id=$2 AND revoked_at IS NULL AND expires_at>now() FOR UPDATE")
        .bind(user).bind(sid).fetch_optional(c).await?.ok_or(E::Permission.into())
}
#[derive(Clone)]
pub struct LegacyProfile {
    pub id: Uuid,
    pub nickname: String,
    pub created_at: i64,
}
impl Database {
    pub async fn legacy_profile(&self, legacy: LegacyProfile) -> Result<Profile, E> {
        self.transaction(move |c| {let l=legacy.clone();Box::pin(async move {
            sqlx::query("INSERT INTO patchwork.users(user_id,nickname,created_at) VALUES($1,$2,to_timestamp($3)) ON CONFLICT DO NOTHING")
                .bind(l.id).bind(l.nickname).bind(l.created_at as f64).execute(&mut *c).await?;
            let row=locked_user(c,l.id).await?;
            if row.get::<bool,_>("legacy_exchanged") || row.get::<i64,_>("auth_version")!=0 {return Err(E::Permission.into());}
            sqlx::query("INSERT INTO patchwork.player_occupancy(user_id) VALUES($1) ON CONFLICT DO NOTHING").bind(l.id).execute(c).await?;
            Ok(profile(&row))
        })}).await.map(|v|v.into_inner())
    }
    /// Stable client-generated session ID and candidate hash make a lost response retryable.
    pub async fn create_identity_session(
        &self,
        sid: Uuid,
        hash: Vec<u8>,
        legacy: Option<LegacyProfile>,
    ) -> Result<Profile, E> {
        let new_id = Uuid::new_v4();
        let nickname = format!("玩家_{:04}", rand::random_range(0..10000));
        self.transaction(move |c| {let hash=hash.clone();let legacy=legacy.clone();let nickname=nickname.clone();Box::pin(async move {
            // Serialize even before a new session row exists. No other operation takes this lock after a user lock.
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))").bind(sid.to_string()).execute(&mut *c).await?;
            if let Some(row)=sqlx::query("SELECT user_id,refresh_hash,revoked_at,expires_at FROM patchwork.sessions WHERE session_id=$1")
                .bind(sid).fetch_optional(&mut *c).await? {
                if row.get::<Vec<u8>,_>("refresh_hash")!=hash || row.get::<Option<DateTime<Utc>>,_>("revoked_at").is_some() || row.get::<DateTime<Utc>,_>("expires_at")<=Utc::now()
                    || legacy.as_ref().is_some_and(|l|l.id!=row.get::<Uuid,_>("user_id")) {return Err(E::Permission.into());}
                let id=row.get("user_id");let row=locked_user(c,id).await?;
                let session=active_session(c,id,sid).await?;
                // A concurrent rotation can commit while we wait for the user lock.
                if session.get::<Vec<u8>,_>("refresh_hash")!=hash || (legacy.is_some() && row.get::<i64,_>("auth_version")!=0) {return Err(E::Permission.into());}
                return Ok(profile(&row));
            }
            let id=legacy.as_ref().map_or(new_id,|l|l.id);
            let name=legacy.as_ref().map_or(nickname,|l|l.nickname.clone());
            let created=legacy.as_ref().map_or(Utc::now().timestamp(),|l|l.created_at);
            sqlx::query("INSERT INTO patchwork.users(user_id,nickname,created_at) VALUES($1,$2,to_timestamp($3)) ON CONFLICT DO NOTHING")
                .bind(id).bind(name).bind(created as f64).execute(&mut *c).await?;
            let row=locked_user(c,id).await?;
            if legacy.is_some() {
                if row.get::<bool,_>("legacy_exchanged") || row.get::<i64,_>("auth_version")!=0 {return Err(E::Permission.into());}
                sqlx::query("UPDATE patchwork.users SET legacy_exchanged=true WHERE user_id=$1").bind(id).execute(&mut *c).await?;
            }
            sqlx::query("INSERT INTO patchwork.player_occupancy(user_id) VALUES($1) ON CONFLICT DO NOTHING").bind(id).execute(&mut *c).await?;
            sqlx::query("INSERT INTO patchwork.sessions(session_id,user_id,refresh_hash,expires_at) VALUES($1,$2,$3,now()+interval '30 days')")
                .bind(sid).bind(id).bind(hash).execute(c).await?;
            Ok(profile(&row))
        })}).await.map(|v|v.into_inner())
    }
    pub async fn session_profile(&self, user: Uuid, sid: Uuid, version: i64) -> Result<Profile, E> {
        let row=sqlx::query("SELECT u.* FROM patchwork.users u JOIN patchwork.sessions s USING(user_id) WHERE u.user_id=$1 AND s.session_id=$2 AND u.auth_version=$3 AND s.revoked_at IS NULL AND s.expires_at>now()")
            .bind(user).bind(sid).bind(version).fetch_optional(self.pool()).await.map_err(|_|E::Unavailable)?.ok_or(E::Permission)?;
        Ok(profile(&row))
    }
    pub async fn rotate_session(
        &self,
        sid: Uuid,
        old: Vec<u8>,
        next: Vec<u8>,
        rotation: Uuid,
    ) -> Result<Profile, E> {
        if old == next {
            return Err(E::InvalidInput);
        }
        let user: Uuid =
            sqlx::query_scalar("SELECT user_id FROM patchwork.sessions WHERE session_id=$1")
                .bind(sid)
                .fetch_optional(self.pool())
                .await
                .map_err(|_| E::Unavailable)?
                .ok_or(E::Permission)?;
        self.transaction(move |c| {let old=old.clone();let next=next.clone();Box::pin(async move {
            let row=locked_user(c,user).await?;let s=active_session(c,user,sid).await?;
            let current:Vec<u8>=s.get("refresh_hash");
            if current==next && s.get::<Option<Vec<u8>>,_>("previous_refresh_hash")==Some(old.clone()) && s.get::<Option<Uuid>,_>("rotation_id")==Some(rotation) {return Ok(profile(&row));}
            if current!=old || s.get::<Option<Uuid>,_>("rotation_id")==Some(rotation) {return Err(E::Permission.into());}
            sqlx::query("UPDATE patchwork.sessions SET previous_refresh_hash=refresh_hash,refresh_hash=$2,rotation_id=$3,expires_at=now()+interval '30 days' WHERE session_id=$1")
                .bind(sid).bind(next).bind(rotation).execute(c).await?;
            Ok(profile(&row))
        })}).await.map(|v|v.into_inner())
    }
    pub async fn rename_identity(
        &self,
        user: Uuid,
        sid: Option<Uuid>,
        version: i64,
        name: String,
    ) -> Result<Profile, E> {
        self.transaction(move |c| {
            let name = name.clone();
            Box::pin(async move {
                let row = locked_user(c, user).await?;
                if row.get::<i64, _>("auth_version") != version {
                    return Err(E::Permission.into());
                }
                if let Some(sid) = sid {
                    active_session(c, user, sid).await?;
                } else if row.get::<bool, _>("legacy_exchanged") {
                    return Err(E::Permission.into());
                }
                let row = sqlx::query(
                    "UPDATE patchwork.users SET nickname=$2 WHERE user_id=$1 RETURNING *",
                )
                .bind(user)
                .bind(name)
                .fetch_one(c)
                .await?;
                Ok(profile(&row))
            })
        })
        .await
        .map(|v| v.into_inner())
    }
    pub async fn revoke_session(&self, user: Uuid, sid: Uuid) -> Result<(), E> {
        self.transaction(move |c|Box::pin(async move {
            locked_user(c,user).await?;
            sqlx::query("UPDATE patchwork.sessions SET revoked_at=coalesce(revoked_at,now()) WHERE user_id=$1 AND session_id=$2").bind(user).bind(sid).execute(c).await?;Ok(())
        })).await.map(|_|())
    }
    pub async fn claim_connection(&self, user: Uuid, sid: Uuid, version: i64) -> Result<u64, E> {
        self.transaction(move |c|Box::pin(async move {
            let row=locked_user(c,user).await?;
            if row.get::<i64,_>("auth_version")!=version {return Err(E::Permission.into());}
            active_session(c,user,sid).await?;
            let generation:i64=sqlx::query_scalar("UPDATE patchwork.users SET connection_generation=connection_generation+1 WHERE user_id=$1 AND connection_generation<9223372036854775807 RETURNING connection_generation")
                .bind(user).fetch_optional(c).await?.ok_or(E::Unavailable)?;
            Ok(generation as u64)
        })).await.map(|v|v.into_inner())
    }
}
