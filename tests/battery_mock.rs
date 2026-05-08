use mxbattery::battery::{mock::MockBackend, BatteryBackend, BatteryEvent};
use mxbattery::state::ChargingState;
use std::time::Duration;

#[tokio::test]
async fn mock_emits_recorded_events() {
    let mut events = vec![
        BatteryEvent::Connected {
            name: "MX Master 3 Mac".into(),
        },
        BatteryEvent::Percent(17),
        BatteryEvent::Charging(ChargingState::Recharging),
        BatteryEvent::Disconnected,
    ];
    events.reverse();
    let backend = MockBackend::new(events);
    let mut rx = backend.subscribe();

    backend.run_to_completion().await;

    let collected: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert_eq!(collected.len(), 4);
    matches!(collected[0], BatteryEvent::Connected { .. });
    matches!(collected[1], BatteryEvent::Percent(17));
    matches!(
        collected[2],
        BatteryEvent::Charging(ChargingState::Recharging)
    );
    matches!(collected[3], BatteryEvent::Disconnected);

    let _ = Duration::from_millis(0);
}
