use super::*;

use serial_test::serial;

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn connect_through_handle_prompt_surfaces_output_without_sampling() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let home = tempfile::TempDir::new().expect("temporary GROK_HOME");
            // SAFETY: this test is serialized because GROK_HOME is process-global.
            unsafe { std::env::set_var("GROK_HOME", home.path()) };

            let (gateway_tx, _gateway_rx) =
                tokio::sync::mpsc::unbounded_channel::<xai_acp_lib::AcpClientMessage>();
            let (persistence_tx, mut persistence_rx) =
                tokio::sync::mpsc::unbounded_channel::<PersistenceMsg>();
            let (actor, mut event_rx) =
                support::create_test_actor_ex(0, 256_000, 85, gateway_tx, persistence_tx).await;
            let actor = std::sync::Arc::new(actor);
            let surfaced = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
            let surfaced_task = std::sync::Arc::clone(&surfaced);
            tokio::task::spawn_local(async move {
                while let Some(event) = event_rx.recv().await {
                    match event {
                        crate::session::replay_events::SessionEvent::Notification(
                            crate::session::replay_events::SessionNotification::Acp(notification),
                        ) => {
                            if let acp::SessionUpdate::AgentMessageChunk(chunk) =
                                notification.update
                                && let acp::ContentBlock::Text(text) = chunk.content
                            {
                                surfaced_task.lock().push(text.text);
                            }
                        }
                        crate::session::replay_events::SessionEvent::FlushReplay { respond_to } => {
                            if let Some(tx) = respond_to {
                                let _ = tx.send(());
                            }
                        }
                        crate::session::replay_events::SessionEvent::Notification(_) => {}
                    }
                }
            });

            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                actor.handle_prompt(
                    "connect-command-test",
                    vec![acp::ContentBlock::Text(acp::TextContent::new("/connect"))],
                    PromptMode::Agent,
                    None,
                    None,
                    None,
                    None,
                    false,
                    None,
                    None,
                    None,
                ),
            )
            .await
            .expect("/connect must be intercepted instead of waiting on the sampler");
            assert!(result.is_ok(), "command should end cleanly: {result:?}");
            tokio::task::yield_now().await;

            assert!(
                surfaced
                    .lock()
                    .iter()
                    .any(|text| text.contains("No accounts configured")),
                "/connect output must arrive as an ACP agent message chunk"
            );
            while let Ok(message) = persistence_rx.try_recv() {
                if let PersistenceMsg::Update(crate::session::storage::SessionUpdate::Acp(
                    notification,
                )) = message
                {
                    assert!(
                        !matches!(notification.update, acp::SessionUpdate::UserMessageChunk(_)),
                        "/connect must not be persisted as sampler input"
                    );
                }
            }

            // SAFETY: paired with the serialized set_var above.
            unsafe { std::env::remove_var("GROK_HOME") };
        })
        .await;
}
