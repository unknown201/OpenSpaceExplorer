//! Email OTP credentials stay in memory, expire after ten minutes and are consumed on success.
use lettre::{Message, SmtpTransport, Transport};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, net::IpAddr, sync::Mutex, time::Duration};

pub const TTL: u64 = 600;
pub const RESEND: u64 = 60;
const ATTEMPTS: u8 = 5;
struct Challenge {
    id: String,
    hash: [u8; 32],
    target: Option<u64>,
    expires: u64,
    attempts: u8,
}
#[derive(Default)]
struct State {
    challenges: HashMap<String, Challenge>,
    sent: Vec<(u64, String, IpAddr)>,
}
pub struct Otp {
    state: Mutex<State>,
    mail: Option<(SmtpTransport, String)>,
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn normalize(email: &str) -> Result<String, String> {
    let email = email.trim().to_ascii_lowercase();
    if email.len() > 254 || email.parse::<lettre::Address>().is_err() {
        return Err("Enter a valid email address.".into());
    }
    Ok(email)
}
fn digest(id: &str, code: &str) -> [u8; 32] {
    Sha256::digest(format!("{id}:{code}")).into()
}
fn code() -> Result<String, String> {
    loop {
        let mut bytes = [0; 4];
        getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
        let n = u32::from_le_bytes(bytes);
        if n < 4_294_000_000 {
            return Ok(format!("{:06}", n % 1_000_000));
        }
    }
}
impl Otp {
    pub fn from_env() -> anyhow::Result<Self> {
        let mail = match std::env::var("OSE_SMTP_HOST") {
            Ok(host) => {
                let from = std::env::var("OSE_SMTP_FROM")?;
                let _: lettre::message::Mailbox = from.parse()?;
                let port = std::env::var("OSE_SMTP_PORT")
                    .unwrap_or_else(|_| "25".into())
                    .parse()?;
                // The internal, source-restricted relay handles outbound authenticated TLS delivery.
                Some((
                    SmtpTransport::builder_dangerous(host)
                        .port(port)
                        .timeout(Some(Duration::from_secs(10)))
                        .build(),
                    from,
                ))
            }
            Err(_) => None,
        };
        Ok(Self {
            state: Mutex::default(),
            mail,
        })
    }
    pub async fn request(
        &self,
        email: &str,
        target: Option<u64>,
        ip: IpAddr,
    ) -> Result<(String, String), String> {
        let email = normalize(email)?;
        let (mail, from) = self
            .mail
            .as_ref()
            .ok_or("Email sign-in is not configured. Try again later.")?;
        let (id, code) = self.issue(&email, target, ip, now())?;
        let message = Message::builder().from(from.parse().map_err(|_| "Email sender unavailable")?)
            .to(email.parse().map_err(|_| "Invalid email address")?)
            .subject("Your OpenSpaceExplorer sign-in code")
            .body(format!("Your OpenSpaceExplorer code is: {code}\n\nIt expires in 10 minutes and can be used once.\nIf you did not request this code, ignore this email.\n\nhttps://ose.jfly.dev/\n"))
            .map_err(|_| "Could not prepare sign-in email")?;
        let mail = mail.clone();
        let delivered = tokio::task::spawn_blocking(move || mail.send(&message)).await;
        if !matches!(delivered, Ok(Ok(_))) {
            self.state
                .lock()
                .expect("otp lock")
                .challenges
                .remove(&email);
            tracing::warn!("OTP email delivery failed");
            return Err("Could not send the code. Please try again shortly.".into());
        }
        Ok((email, id))
    }
    fn issue(
        &self,
        email: &str,
        target: Option<u64>,
        ip: IpAddr,
        now: u64,
    ) -> Result<(String, String), String> {
        let mut s = self.state.lock().expect("otp lock");
        s.challenges.retain(|_, c| c.expires > now);
        s.sent.retain(|(t, _, _)| now.saturating_sub(*t) < 3600);
        if s.sent
            .iter()
            .any(|(t, e, _)| e == email && now.saturating_sub(*t) < RESEND)
        {
            return Err("Please wait 60 seconds before requesting another code.".into());
        }
        if s.sent.len() >= 200
            || s.sent.iter().filter(|(_, e, _)| e == email).count() >= 8
            || s.sent.iter().filter(|(_, _, i)| *i == ip).count() >= 30
        {
            return Err("Too many code requests. Please try again later.".into());
        }
        let id = crate::accounts::random_token().map_err(|e| e.to_string())?;
        let code = code()?;
        s.challenges.insert(
            email.into(),
            Challenge {
                id: id.clone(),
                hash: digest(&id, &code),
                target,
                expires: now + TTL,
                attempts: ATTEMPTS,
            },
        );
        s.sent.push((now, email.into(), ip));
        Ok((id, code))
    }
    pub fn verify(
        &self,
        email: &str,
        id: &str,
        code: &str,
        target: Option<u64>,
    ) -> Result<String, String> {
        self.verify_at(email, id, code, target, now())
    }
    fn verify_at(
        &self,
        email: &str,
        id: &str,
        code: &str,
        target: Option<u64>,
        now: u64,
    ) -> Result<String, String> {
        let email = normalize(email)?;
        let mut s = self.state.lock().expect("otp lock");
        let c = s
            .challenges
            .get_mut(&email)
            .ok_or("Request a new code first.")?;
        if c.expires <= now || c.attempts == 0 {
            s.challenges.remove(&email);
            return Err("Code expired. Request a new code.".into());
        }
        if c.id != id || c.target != target {
            return Err("Request a new code on this screen.".into());
        }
        c.attempts -= 1;
        let hash = digest(id, code.trim());
        let equal = c
            .hash
            .iter()
            .zip(hash)
            .fold(0u8, |acc, (a, b)| acc | (*a ^ b))
            == 0;
        if !equal {
            return Err("Incorrect code. Check the email and try again.".into());
        }
        s.challenges.remove(&email);
        Ok(email)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codes_expire_are_single_use_and_bound_to_the_account() {
        let otp = Otp {
            state: Mutex::default(),
            mail: None,
        };
        let ip = "127.0.0.1".parse().unwrap();
        let (id, code) = otp.issue("pilot@example.com", Some(7), ip, 100).unwrap();
        assert!(otp
            .verify_at("pilot@example.com", &id, &code, None, 101)
            .is_err());
        assert!(otp
            .verify_at("pilot@example.com", &id, &code, Some(8), 101)
            .is_err());
        assert!(otp
            .verify_at("pilot@example.com", &id, &code, Some(7), 101)
            .is_ok());
        assert!(otp
            .verify_at("pilot@example.com", &id, &code, Some(7), 102)
            .is_err());
        let (id, code) = otp.issue("pilot@example.com", None, ip, 160).unwrap();
        assert!(otp
            .verify_at("pilot@example.com", &id, &code, None, 761)
            .is_err());
    }
    #[test]
    fn retries_and_resends_are_limited_and_old_codes_stop_working() {
        let otp = Otp {
            state: Mutex::default(),
            mail: None,
        };
        let ip = "127.0.0.1".parse().unwrap();
        let (id, code) = otp.issue("pilot@example.com", None, ip, 100).unwrap();
        assert!(otp.issue("pilot@example.com", None, ip, 110).is_err());
        for _ in 0..ATTEMPTS {
            assert!(otp
                .verify_at("pilot@example.com", &id, "bad", None, 111)
                .is_err());
        }
        assert!(otp
            .verify_at("pilot@example.com", &id, &code, None, 111)
            .is_err());
        let (old_id, old_code) = otp.issue("pilot@example.com", None, ip, 160).unwrap();
        let (id, code) = otp.issue("pilot@example.com", None, ip, 220).unwrap();
        assert!(otp
            .verify_at("pilot@example.com", &old_id, &old_code, None, 221)
            .is_err());
        assert!(otp
            .verify_at("pilot@example.com", &id, &code, None, 221)
            .is_ok());
        assert!(normalize("pilot@example.com\r\nBcc: bad@example.com").is_err());
    }
}
