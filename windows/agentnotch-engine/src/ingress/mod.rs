//! Hook ingress (HS§4, DESIGN-WIN §1.4 "Server dispatch"): lenient decoding
//! of frames, ignored sessions, the ToolUseIdCache, held PermissionRequests
//! (answer, release, peer gone) and control requests.
//!
//! Owner: WP1. WP0 stub: the §3.4 signatures; it drops every frame.

use crate::model::SessionId;
use crate::platform::TransportEvent;
use crate::runtime_types::{AnswerResult, IngressConfig, IngressOut, Release};
use agentnotch_proto::PermissionResponse;
use std::time::SystemTime;

pub struct HookIngress {
    cfg: IngressConfig,
}

impl HookIngress {
    pub fn new(cfg: IngressConfig) -> Self {
        HookIngress { cfg }
    }

    pub fn config(&self) -> &IngressConfig {
        &self.cfg
    }

    pub fn on_transport(&mut self, ev: TransportEvent, now: SystemTime) -> Vec<IngressOut> {
        let _ = (ev, now);
        Vec::new()
    }

    pub fn answer(
        &mut self,
        session: &SessionId,
        tool_use_id: &str,
        r: PermissionResponse,
    ) -> AnswerResult {
        let _ = (session, tool_use_id, r);
        AnswerResult::NotPending
    }

    /// Closes held requests without answering.
    pub fn release(&mut self, which: Release) {
        let _ = which;
    }

    pub fn pending(&self) -> Vec<(SessionId, String)> {
        Vec::new()
    }
}
