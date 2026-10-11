//! Hard safety constraints for measurement and, above all, for energising a
//! physical board.
//!
//! Three layers, deliberately independent of each other and of the caller:
//!
//! 1. [`MAX_VOLTAGE_V`] / [`MAX_CURRENT_A`] caps, checked before any backend call.
//! 2. [`ConfirmToken`]: a single-use token that only exists if a human confirmed
//!    *this* operation. It is the explicit, auditable form of "the user said yes".
//! 3. [`source_enabled`](crate::hal::source_enabled): a global opt-in. Even with a
//!    valid token and in-range values, nothing energises unless the operator has
//!    enabled it for the session.
//!
//! Scope note (stated honestly, see `design.md` D3): an in-band token cannot stop
//! a *malicious* agent, because any value it must supply is visible to it. The
//! non-bypassable parts are the caps, the default-off opt-in, and the fuse — those
//! are enforced in code on every path. The token's job is to make confirmation
//! explicit and to stop accidental energising.

use crate::hal::{Instrument, InstrumentError, MeasureRequest, Measurement, SourceRequest};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Project ceiling for DC voltage (PRD §7.3.4).
pub const MAX_VOLTAGE_V: f64 = 48.0;
/// Project ceiling for current (PRD §7.3.4).
pub const MAX_CURRENT_A: f64 = 5.0;
/// A measurement above this multiple of the expectation trips the fuse.
pub const FUSE_MULTIPLE: f64 = 2.0;

/// Why an operation was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SafetyViolation {
    #[error("请求电压 {requested} V 超过上限 {limit} V")]
    VoltageLimit { requested: f64, limit: f64 },
    #[error("请求电流 {requested} A 超过上限 {limit} A")]
    CurrentLimit { requested: f64, limit: f64 },
    #[error("带电操作需要用户当轮确认，但未提供确认令牌")]
    MissingConfirmation,
    #[error("确认令牌无效或已被使用（一次性）")]
    TokenAlreadyUsed,
    #[error("本次会话未启用源输出（需显式设置 GCODE_ALLOW_SOURCE=1）")]
    SourceDisabled,
}

impl SafetyViolation {
    /// Actionable next step for the user/agent.
    pub fn hint(&self) -> &'static str {
        match self {
            SafetyViolation::VoltageLimit { .. } => {
                "降低目标电压至 48V 以内；市电/高压电路不在支持范围内。"
            }
            SafetyViolation::CurrentLimit { .. } => {
                "降低限流值至 5A 以内；大电流功率测试不在支持范围内。"
            }
            SafetyViolation::MissingConfirmation => {
                "请先向用户说明将要上电的电压/限流并取得确认，再以 user_confirmed=true 重试。"
            }
            SafetyViolation::TokenAlreadyUsed => {
                "每次带电操作都需要一次新的用户确认；请重新取得确认。"
            }
            SafetyViolation::SourceDisabled => {
                "如需允许硬件上电，请在启动前设置 GCODE_ALLOW_SOURCE=1（默认禁止）。"
            }
        }
    }

    /// Whether this refusal is about confirmation rather than physical limits.
    pub fn needs_user_confirmation(&self) -> bool {
        matches!(
            self,
            SafetyViolation::MissingConfirmation | SafetyViolation::TokenAlreadyUsed
        )
    }
}

/// Proof that the user confirmed one energising session (one test run).
///
/// A token can arm exactly one session; every energising request inside that
/// session is then authorised. A second session needs a new token.
/// Not `Clone`/`Deserialize` on purpose: it cannot be duplicated nor
/// reconstructed from stored data.
#[derive(Debug)]
pub struct ConfirmToken {
    id: u64,
    user_confirmed: bool,
    armed: AtomicBool,
}

static NEXT_TOKEN_ID: AtomicU64 = AtomicU64::new(1);

impl ConfirmToken {
    /// Mint a token. Returns `None` when the caller did not assert that the user
    /// confirmed this operation — i.e. an agent cannot obtain one by guessing.
    pub fn mint(user_confirmed: bool) -> Option<Self> {
        if !user_confirmed {
            return None;
        }
        Some(Self {
            id: NEXT_TOKEN_ID.fetch_add(1, Ordering::SeqCst),
            user_confirmed: true,
            armed: AtomicBool::new(false),
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst)
    }

    /// Arm the session. Succeeds once per token; records the audit point.
    pub fn arm(&self) -> Result<(), SafetyViolation> {
        if !self.user_confirmed {
            return Err(SafetyViolation::MissingConfirmation);
        }
        if self.armed.swap(true, Ordering::SeqCst) {
            return Err(SafetyViolation::TokenAlreadyUsed);
        }
        Ok(())
    }

    /// Authorise an energising request: requires an armed, user-backed token.
    pub fn authorise(&self) -> Result<(), SafetyViolation> {
        if !self.user_confirmed {
            return Err(SafetyViolation::MissingConfirmation);
        }
        if !self.is_armed() {
            return Err(SafetyViolation::MissingConfirmation);
        }
        Ok(())
    }
}

/// Check the hard limits on a source request.
pub fn check_limits(req: &SourceRequest) -> Result<(), SafetyViolation> {
    if let Some(v) = req.voltage {
        if v.abs() > MAX_VOLTAGE_V {
            return Err(SafetyViolation::VoltageLimit {
                requested: v,
                limit: MAX_VOLTAGE_V,
            });
        }
    }
    if let Some(c) = req.current_limit {
        if c.abs() > MAX_CURRENT_A {
            return Err(SafetyViolation::CurrentLimit {
                requested: c,
                limit: MAX_CURRENT_A,
            });
        }
    }
    Ok(())
}

/// Whether a measured current should trip the fuse.
///
/// Only meaningful with a positive expectation; a zero expectation cannot define
/// "twice as much" and must not trip on any reading.
pub fn fuse_tripped(measured: f64, expected: f64) -> bool {
    expected > 0.0 && measured > FUSE_MULTIPLE * expected
}

/// Authorise an energising request: opt-in, limits, then confirmation.
///
/// Shared by [`SafeInstrument`] and the executor, so safety does not depend on
/// the caller having remembered to wrap the backend (defence in depth).
pub fn authorise(
    req: &SourceRequest,
    auth: Option<&ConfirmToken>,
) -> Result<(), SafetyViolation> {
    if !crate::hal::source_enabled() {
        return Err(SafetyViolation::SourceDisabled);
    }
    check_limits(req)?;
    if req.energises() {
        let token = auth.ok_or(SafetyViolation::MissingConfirmation)?;
        if token.is_armed() {
            token.authorise()?;
        } else {
            token.arm()?;
        }
    }
    Ok(())
}

/// Wraps any instrument and enforces the safety rules on every call.
pub struct SafeInstrument<I> {
    inner: I,
}

impl<I: Instrument> SafeInstrument<I> {
    pub fn new(inner: I) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> &I {
        &self.inner
    }
}

#[async_trait]
impl<I: Instrument> Instrument for SafeInstrument<I> {
    fn id(&self) -> String {
        self.inner.id()
    }

    fn capabilities(&self) -> crate::hal::Capabilities {
        self.inner.capabilities()
    }

    async fn measure(&self, req: &MeasureRequest) -> Result<Measurement, InstrumentError> {
        // Measuring is passive: no token needed. Range sanity for measurements
        // lives with the plan/expectation layer, which knows the intent.
        self.inner.measure(req).await
    }

    async fn source(
        &self,
        req: &SourceRequest,
        auth: Option<&ConfirmToken>,
    ) -> Result<(), InstrumentError> {
        authorise(req, auth).map_err(|v| InstrumentError::Rejected {
            reason: v.to_string(),
            hint: v.hint().to_string(),
        })?;
        self.inner.source(req, auth).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hal::{Capabilities, Quantity};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        sources: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Instrument for Recorder {
        fn id(&self) -> String {
            "recorder".into()
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                quantities: vec![Quantity::VoltageDc],
                max_voltage: MAX_VOLTAGE_V,
                max_current: MAX_CURRENT_A,
                can_source: true,
            }
        }
        async fn measure(&self, req: &MeasureRequest) -> Result<Measurement, InstrumentError> {
            Ok(Measurement::new(req.quantity, 1.0, "recorder", "t", false))
        }
        async fn source(
            &self,
            req: &SourceRequest,
            _auth: Option<&ConfirmToken>,
        ) -> Result<(), InstrumentError> {
            self.sources.lock().unwrap().push(format!(
                "V={:?} I={:?} OUT={:?}",
                req.voltage, req.current_limit, req.output
            ));
            Ok(())
        }
    }

    fn enable_source() {
        // SAFETY: the environment flag is process-global; the single
        // environment-dependent test below sets and restores it.
        unsafe { std::env::set_var(crate::hal::ALLOW_SOURCE_ENV, "1") };
    }

    fn disable_source() {
        unsafe { std::env::remove_var(crate::hal::ALLOW_SOURCE_ENV) };
    }

    #[test]
    fn limits_reject_out_of_range() {
        assert!(matches!(
            check_limits(&SourceRequest::set_voltage(60.0)),
            Err(SafetyViolation::VoltageLimit { .. })
        ));
        assert!(matches!(
            check_limits(&SourceRequest::set_voltage(5.0).with_current_limit(6.0)),
            Err(SafetyViolation::CurrentLimit { .. })
        ));
    }

    #[test]
    fn limits_allow_the_boundary() {
        assert!(check_limits(&SourceRequest::set_voltage(48.0)).is_ok());
        assert!(check_limits(&SourceRequest::set_voltage(5.0).with_current_limit(5.0)).is_ok());
    }

    #[test]
    fn token_requires_user_confirmation() {
        assert!(ConfirmToken::mint(false).is_none(), "agent cannot self-mint");
        assert!(ConfirmToken::mint(true).is_some());
    }

    #[test]
    fn token_arms_exactly_one_session() {
        let t = ConfirmToken::mint(true).unwrap();
        assert_eq!(t.authorise(), Err(SafetyViolation::MissingConfirmation));
        assert!(t.arm().is_ok());
        assert!(t.is_armed());
        assert!(t.authorise().is_ok());
        assert_eq!(t.arm(), Err(SafetyViolation::TokenAlreadyUsed));
    }

    #[test]
    fn fuse_only_trips_above_twice_the_expectation() {
        assert!(!fuse_tripped(0.19, 0.1));
        assert!(!fuse_tripped(0.2, 0.1));
        assert!(fuse_tripped(0.21, 0.1));
        assert!(!fuse_tripped(5.0, 0.0), "a zero expectation cannot define a multiple");
    }

    #[test]
    fn violations_are_actionable() {
        assert!(SafetyViolation::VoltageLimit { requested: 60.0, limit: 48.0 }
            .hint()
            .contains("48V"));
        assert!(SafetyViolation::MissingConfirmation.needs_user_confirmation());
        assert!(!SafetyViolation::VoltageLimit { requested: 60.0, limit: 48.0 }
            .needs_user_confirmation());
    }

    /// The opt-in flag is process-global, so every environment-dependent case
    /// runs in ONE test, sequentially. Parallel tests here would race.
    #[tokio::test]
    async fn source_gate_behaviour() {
        // The flag is process-global: hold the env lock for the whole test.
        let _guard = crate::test_support::env_lock();
        // 1) Disabled by default: refused, with the opt-in hint.
        disable_source();
        let safe = SafeInstrument::new(Recorder::default());
        let token = ConfirmToken::mint(true).unwrap();
        match safe.source(&SourceRequest::set_voltage(3.3), Some(&token)).await {
            Err(InstrumentError::Rejected { reason, hint }) => {
                assert!(reason.contains("未启用"), "{reason}");
                assert!(hint.contains(crate::hal::ALLOW_SOURCE_ENV));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }

        // 2) Enabled, but no token: refused as needing confirmation.
        enable_source();
        match safe.source(&SourceRequest::set_voltage(3.3), None).await {
            Err(InstrumentError::Rejected { reason, hint }) => {
                assert!(reason.contains("确认"), "{reason}");
                assert!(hint.contains("确认"));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert!(
            safe.inner().sources.lock().unwrap().is_empty(),
            "nothing may reach the backend without confirmation"
        );

        // 3) Enabled and confirmed, but over the cap: refused.
        match safe.source(&SourceRequest::set_voltage(60.0), Some(&token)).await {
            Err(InstrumentError::Rejected { reason, .. }) => {
                assert!(reason.contains("上限") || reason.contains("48"), "{reason}");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert!(safe.inner().sources.lock().unwrap().is_empty());

        // 4) Enabled, confirmed, in range: allowed and recorded.
        let res = safe.source(&SourceRequest::set_voltage(3.3), Some(&token)).await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(safe.inner().sources.lock().unwrap().len(), 1);
        assert!(token.is_armed(), "the first energising call arms the session");

        // 5) Within the armed session further energising requests are allowed.
        let res = safe
            .source(&SourceRequest::set_voltage(3.4).with_current_limit(0.5), Some(&token))
            .await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(safe.inner().sources.lock().unwrap().len(), 2);

        // 6) A passive request (output off) needs no token.
        let res = safe.source(&SourceRequest::set_voltage(0.0).output_on(false), None).await;
        assert!(res.is_ok(), "{res:?}");

        // 7) A second session needs a new token.
        let t2 = ConfirmToken::mint(true).unwrap();
        assert!(t2.arm().is_ok());
        assert_eq!(t2.arm(), Err(SafetyViolation::TokenAlreadyUsed));

        disable_source();
    }
}
