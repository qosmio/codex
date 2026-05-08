use std::collections::HashSet;
use std::sync::Arc;

use crate::Prompt;
use crate::ResponseStream;
use crate::client::CompactConversationRequestSettings;
use crate::client::ModelClientSession;
use crate::client_common::ResponseEvent;
use crate::compact::CompactionAnalyticsAttempt;
use crate::compact::InitialContextInjection;
use crate::compact::compaction_status_from_result;
use crate::compact_remote::build_compact_request_log_data;
use crate::compact_remote::log_remote_compact_failure;
use crate::compact_remote::process_compacted_history;
use crate::compact_remote::trim_function_call_history_to_fit_context_window;
use crate::session::session::Session;
use crate::session::turn::built_tools;
use crate::session::turn_context::TurnContext;
use codex_analytics::CompactionImplementation;
use codex_analytics::CompactionPhase;
use codex_analytics::CompactionReason;
use codex_analytics::CompactionTrigger;
use codex_features::Feature;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::items::ContextCompactionItem;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CompactedItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TurnStartedEvent;
use codex_rollout_trace::CompactionCheckpointTracePayload;
use codex_rollout_trace::InferenceTraceContext;
use futures::StreamExt;
use futures::TryFutureExt;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing::warn;

pub(crate) async fn run_inline_remote_auto_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    client_session: &mut ModelClientSession,
    initial_context_injection: InitialContextInjection,
    reason: CompactionReason,
    phase: CompactionPhase,
) -> CodexResult<()> {
    run_remote_compact_task_inner(
        &sess,
        &turn_context,
        Some(client_session),
        initial_context_injection,
        CompactionTrigger::Auto,
        reason,
        phase,
    )
    .await
}

pub(crate) async fn run_remote_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
) -> CodexResult<()> {
    let start_event = EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: turn_context.sub_id.clone(),
        started_at: turn_context.turn_timing_state.started_at_unix_secs().await,
        model_context_window: turn_context.model_context_window(),
        collaboration_mode_kind: turn_context.collaboration_mode.mode,
    });
    sess.send_event(&turn_context, start_event).await;

    run_remote_compact_task_inner(
        &sess,
        &turn_context,
        /*client_session*/ None,
        InitialContextInjection::DoNotInject,
        CompactionTrigger::Manual,
        CompactionReason::UserRequested,
        CompactionPhase::StandaloneTurn,
    )
    .await
}

async fn run_remote_compact_task_inner(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    client_session: Option<&mut ModelClientSession>,
    initial_context_injection: InitialContextInjection,
    trigger: CompactionTrigger,
    reason: CompactionReason,
    phase: CompactionPhase,
) -> CodexResult<()> {
    let attempt = CompactionAnalyticsAttempt::begin(
        sess.as_ref(),
        turn_context.as_ref(),
        trigger,
        reason,
        CompactionImplementation::Responses,
        phase,
    )
    .await;
    let result = run_remote_compact_task_inner_impl(
        sess,
        turn_context,
        client_session,
        initial_context_injection,
    )
    .await;
    attempt
        .track(
            sess.as_ref(),
            compaction_status_from_result(&result),
            result.as_ref().err().map(ToString::to_string),
        )
        .await;
    if let Err(err) = result {
        let event = EventMsg::Error(
            err.to_error_event(Some("Error running remote compact task".to_string())),
        );
        sess.send_event(turn_context, event).await;
        return Err(err);
    }
    Ok(())
}

async fn run_remote_compact_task_inner_impl(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    client_session: Option<&mut ModelClientSession>,
    initial_context_injection: InitialContextInjection,
) -> CodexResult<()> {
    let context_compaction_item = ContextCompactionItem::new();
    let compaction_trace = sess.services.rollout_thread_trace.compaction_trace_context(
        turn_context.sub_id.as_str(),
        context_compaction_item.id.as_str(),
        turn_context.model_info.slug.as_str(),
        turn_context.provider.info().name.as_str(),
    );
    let compaction_item = TurnItem::ContextCompaction(context_compaction_item);
    sess.emit_turn_item_started(turn_context, &compaction_item)
        .await;

    let mut history = sess.clone_history().await;
    let base_instructions = sess.get_base_instructions().await;
    let deleted_items = trim_function_call_history_to_fit_context_window(
        &mut history,
        turn_context.as_ref(),
        &base_instructions,
    );
    if deleted_items > 0 {
        info!(
            turn_id = %turn_context.sub_id,
            deleted_items,
            "trimmed history items before remote compaction v2"
        );
    }

    let trace_input_history = history.raw_items().to_vec();
    let prompt_input = history
        .for_prompt(&turn_context.model_info.input_modalities)
        .into_iter()
        .filter(|item| !matches!(item, ResponseItem::ContextCompaction { .. }))
        .collect::<Vec<_>>();
    let tool_router = built_tools(
        sess.as_ref(),
        turn_context.as_ref(),
        &prompt_input,
        &HashSet::new(),
        /*skills_outcome*/ None,
        &CancellationToken::new(),
    )
    .await?;
    let mut input = prompt_input.clone();
    input.push(ResponseItem::Compaction {
        encrypted_content: String::new(),
    });
    let prompt = Prompt {
        input,
        tools: tool_router.model_visible_specs(),
        parallel_tool_calls: turn_context.model_info.supports_parallel_tool_calls,
        base_instructions,
        personality: turn_context.personality,
        output_schema: None,
        output_schema_strict: true,
    };

    let turn_metadata_header = turn_context.turn_metadata_state.current_header_value();
    let trace_attempt = compaction_trace.start_attempt(&serde_json::json!({
        "model": turn_context.model_info.slug.as_str(),
        "instructions": prompt.base_instructions.text.as_str(),
        "input": &prompt.input,
        "parallel_tool_calls": prompt.parallel_tool_calls,
    }));

    let mut owned_client_session;
    let client_session = match client_session {
        Some(client_session) => client_session,
        None => {
            owned_client_session = sess.services.model_client.new_session();
            &mut owned_client_session
        }
    };
    let compaction_output_result = run_remote_compaction_request_v2(
        turn_context,
        client_session,
        &prompt,
        turn_metadata_header.as_deref(),
    )
    .await;

    let empty_v2_output: &[ResponseItem] = &[];
    trace_attempt.record_result(compaction_output_result.as_ref().map(|_| empty_v2_output));
    let response_id = match compaction_output_result {
        Ok(response_id) => Some(response_id),
        Err(err) => {
            warn!(
                turn_id = %turn_context.sub_id,
                error = %err,
                "remote compaction v2 request failed; falling back to compact endpoint"
            );
            None
        }
    };

    let compact_prompt = Prompt {
        input: prompt_input,
        tools: prompt.tools.clone(),
        parallel_tool_calls: prompt.parallel_tool_calls,
        base_instructions: prompt.base_instructions.clone(),
        personality: prompt.personality,
        output_schema: None,
        output_schema_strict: true,
    };
    let new_history = sess
        .services
        .model_client
        .compact_conversation_history(
            &compact_prompt,
            &turn_context.model_info,
            CompactConversationRequestSettings {
                effort: turn_context.reasoning_effort,
                summary: turn_context.reasoning_summary,
                service_tier: turn_context.config.service_tier.clone(),
            },
            &turn_context.session_telemetry,
            &compaction_trace,
        )
        .or_else(|err| async {
            let total_usage_breakdown = sess.get_total_token_usage_breakdown().await;
            let compact_request_log_data = build_compact_request_log_data(
                &compact_prompt.input,
                &compact_prompt.base_instructions.text,
            );
            log_remote_compact_failure(
                turn_context,
                &compact_request_log_data,
                total_usage_breakdown,
                &err,
            );
            Err(err)
        })
        .await?;
    let new_history = process_compacted_history(
        sess.as_ref(),
        turn_context.as_ref(),
        new_history,
        initial_context_injection,
    )
    .await;

    let reference_context_item = match initial_context_injection {
        InitialContextInjection::DoNotInject => None,
        InitialContextInjection::BeforeLastUserMessage => Some(turn_context.to_turn_context_item()),
    };
    let compacted_item = CompactedItem {
        message: String::new(),
        replacement_history: Some(new_history.clone()),
    };
    compaction_trace.record_installed(&CompactionCheckpointTracePayload {
        input_history: &trace_input_history,
        replacement_history: &new_history,
    });
    sess.replace_compacted_history(new_history, reference_context_item, compacted_item)
        .await;
    sess.recompute_token_usage(turn_context).await;

    sess.emit_turn_item_completed(turn_context, compaction_item)
        .await;
    if let Some(response_id) = response_id
        && turn_context
            .features
            .enabled(Feature::ResponsesWebsocketResponseProcessed)
    {
        client_session.send_response_processed(&response_id).await;
    }
    Ok(())
}

async fn run_remote_compaction_request_v2(
    turn_context: &TurnContext,
    client_session: &mut ModelClientSession,
    prompt: &Prompt,
    turn_metadata_header: Option<&str>,
) -> CodexResult<String> {
    let stream = client_session
        .stream(
            prompt,
            &turn_context.model_info,
            &turn_context.session_telemetry,
            turn_context.reasoning_effort,
            turn_context.reasoning_summary,
            turn_context.config.service_tier.clone(),
            turn_metadata_header,
            &InferenceTraceContext::disabled(),
        )
        .await?;
    collect_compaction_response_id(stream).await
}

async fn collect_compaction_response_id(mut stream: ResponseStream) -> CodexResult<String> {
    while let Some(event) = stream.next().await {
        if let ResponseEvent::Completed { response_id, .. } = event? {
            return Ok(response_id);
        }
    }

    Err(CodexErr::Fatal(
        "remote compaction v2 stream closed before response.completed".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::MessagePhase;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    fn message(role: &str, text: &str, phase: Option<MessagePhase>) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: role.to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            phase,
        }
    }

    fn response_stream(events: Vec<CodexResult<ResponseEvent>>) -> ResponseStream {
        let (tx_event, rx_event) = mpsc::channel(events.len().max(1));
        for event in events {
            tx_event
                .try_send(event)
                .expect("response stream test channel should have capacity");
        }
        drop(tx_event);
        ResponseStream {
            rx_event,
            consumer_dropped: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn collect_compaction_response_id_accepts_additional_output_items() {
        let stream = response_stream(vec![
            Ok(ResponseEvent::OutputItemDone(message(
                "assistant",
                "IGNORED_COMPACT_REPLY",
                Some(MessagePhase::FinalAnswer),
            ))),
            Ok(ResponseEvent::OutputItemDone(
                ResponseItem::ContextCompaction {
                    encrypted_content: Some("encrypted".to_string()),
                },
            )),
            Ok(ResponseEvent::Completed {
                response_id: "resp-compact".to_string(),
                token_usage: None,
                end_turn: Some(true),
            }),
        ]);

        let response_id = collect_compaction_response_id(stream)
            .await
            .expect("response id should be collected");

        assert_eq!(response_id, "resp-compact");
    }
}
