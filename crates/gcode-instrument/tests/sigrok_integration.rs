//! Integration tests against real tooling: `sigrok-cli` (and the absence of a
//! real bench instrument).
//!
//! These skip with a printed reason when sigrok is unavailable, so the offline
//! suite still runs anywhere.

use gcode_instrument::hal::{probe_backends, Instrument, MeasureRequest, Quantity, Reduce};
use gcode_instrument::instruments::{SerialPsu, SigrokInstrument};

#[test]
fn backend_probe_lists_every_backend_with_a_reason() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let statuses = rt.block_on(probe_backends());
    let names: Vec<&str> = statuses.iter().map(|s| s.backend).collect();
    assert_eq!(names, vec!["virtual", "sigrok", "serial-psu"]);
    for s in &statuses {
        assert!(!s.reason.is_empty(), "{} has no reason", s.backend);
        assert!(!s.describe().is_empty());
    }
    // The virtual backend is always available; the others depend on the host.
    assert!(statuses[0].available);
}

#[test]
fn sigrok_reads_the_demo_device_and_flags_it_as_synthetic() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let Ok(instrument) = rt.block_on(SigrokInstrument::probe()) else {
        eprintln!("skip: sigrok-cli (or a device) not available");
        return;
    };

    // This host only has sigrok's synthetic demo device, so the result must be
    // reported as synthetic rather than as a real measurement.
    if instrument.device().synthetic() {
        eprintln!(
            "note: only the sigrok synthetic demo device is present; \
             this validates the acquisition path, not a physical board"
        );
    }

    let req = MeasureRequest::on_channel(Quantity::VoltageDc, "A0")
        .with_samples(64)
        .with_reduce(Reduce::Peak);
    let m = rt.block_on(instrument.measure(&req));
    match m {
        Ok(m) => {
            assert_eq!(m.backend, "sigrok");
            assert_eq!(m.unit, "V");
            // The demo driver emits a +-10 V square wave.
            assert!(
                (m.value - 10.0).abs() < 0.5,
                "expected ~10 V peak from the demo driver, got {}",
                m.value
            );
            assert!(m.synthetic, "demo data must be flagged synthetic");
            assert!(m.source_label().contains("非真实"));
        }
        Err(e) => {
            // A host without any usable device must fail informatively.
            let msg = e.to_string();
            assert!(
                msg.contains("设备") || msg.contains("sigrok"),
                "unexpected error: {msg}"
            );
            eprintln!("skip: measurement failed informatively: {msg}");
        }
    }
}

#[test]
fn sigrok_backend_reports_a_missing_channel_as_a_parse_error() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let Ok(instrument) = rt.block_on(SigrokInstrument::probe()) else {
        eprintln!("skip: sigrok-cli not available");
        return;
    };
    let req = MeasureRequest::on_channel(Quantity::VoltageDc, "NOPE0").with_samples(4);
    match rt.block_on(instrument.measure(&req)) {
        Ok(_) => panic!("a nonexistent channel must not produce a reading"),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("无法解析") || msg.contains("没有通道") || msg.contains("设备"),
                "expected a parse/device error, got: {msg}"
            );
        }
    }
}

#[test]
fn serial_psu_reports_the_missing_port() {
    // Tests never set GCODE_PSU_PORT, so this must fail with an explanation.
    let err = SerialPsu::probe().unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("GCODE_PSU_PORT") || msg.contains("不存在"),
        "unexpected: {msg}"
    );
}
