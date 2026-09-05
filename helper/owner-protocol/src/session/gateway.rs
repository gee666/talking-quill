//! Gateway correlation and authenticated owner-message dispatch.
use super::*;

/// Gateway-side codec for one already-authenticated connection.
pub struct GatewaySessionCodec {
    session: AuthenticatedSession,
    outbound_key: FrameKey,
    next_outbound_sequence: Option<u64>,
    correlations: CorrelationTracker,
    predecessor_route: Option<PredecessorRouteValidator>,
    capture_event_route: Option<CaptureEventRoute>,
    owner_instance_id: Bytes32,
    pending_paste_operations: BTreeMap<u64, Bytes32>,
}

impl fmt::Debug for GatewaySessionCodec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GatewaySessionCodec(<redacted>)")
    }
}

impl GatewaySessionCodec {
    #[must_use]
    pub const fn supports_feature(&self, feature: u64) -> bool {
        self.session.supports_feature(feature)
    }

    /// Creates the post-handshake gateway codec. Gateway request sequence starts
    /// at 1 independently of the owner's authenticated finish.
    pub fn new(
        session: AuthenticatedSession,
        outbound_key: &FrameKey,
    ) -> Result<Self, SessionCodecError> {
        if outbound_key.direction() != Direction::GatewayToOwner {
            return Err(SessionCodecError::Direction);
        }
        let owner_instance_id = session.owner_instance_id();
        Ok(Self {
            session,
            outbound_key: outbound_key.duplicate(),
            next_outbound_sequence: Some(1),
            correlations: CorrelationTracker::new(),
            predecessor_route: None,
            capture_event_route: None,
            owner_instance_id,
            pending_paste_operations: BTreeMap::new(),
        })
    }

    #[must_use]
    pub const fn session_id(&self) -> Bytes32 {
        self.session.session_id()
    }

    #[must_use]
    pub const fn purpose(&self) -> Purpose {
        self.session.purpose()
    }

    #[must_use]
    pub const fn is_test_only(&self) -> bool {
        self.session.is_test_only()
    }

    pub fn encode_request(
        &mut self,
        request: &Request,
    ) -> Result<EncodedRequest, SessionCodecError> {
        if request.method() == crate::Method::FrontAppMetadataGet
            && !self.supports_feature(crate::FRONT_APP_METADATA_V1)
        {
            return Err(SessionCodecError::FeatureNotNegotiated);
        }
        let sequence = self
            .next_outbound_sequence
            .ok_or(SessionCodecError::SequenceExhausted)?;
        self.correlations.register_request(sequence, request)?;
        let body = AuthenticatedEnvelope::request(self.session.session_id(), sequence, request)?
            .encode_body(&self.outbound_key)?;
        let frame = encode_outer_frame(&body)?;
        if let Request::PasteInject(params) = request {
            self.pending_paste_operations
                .insert(sequence, params.operation_id);
        }
        self.next_outbound_sequence = sequence.checked_add(1);
        Ok(EncodedRequest {
            transport_sequence: sequence,
            frame,
        })
    }

    pub fn receive_owner_frame(
        &mut self,
        frame: &[u8],
    ) -> Result<GatewayMessage, SessionCodecError> {
        let body = decode_outer_frame(frame)?;
        let (envelope, message) = self.session.inbound().accept_owner_message(
            body,
            &mut self.correlations,
            self.predecessor_route.as_mut(),
        )?;
        let result = match message {
            OwnerMessage::Response(response) => {
                let correlation = envelope.correlation_sequence();
                if let Response::Success(result) = &response {
                    match result {
                        SuccessResult::LeaseAcquire(acquired) => {
                            self.predecessor_route = Some(PredecessorRouteValidator::new(
                                acquired.capture_lease_id,
                                acquired.capture_lease_epoch.get(),
                            ));
                            self.capture_event_route =
                                Some(CaptureEventRoute::new(acquired.capture_lease_epoch.get()));
                        }
                        SuccessResult::Health(health)
                            if health.owner_instance_id != self.owner_instance_id =>
                        {
                            return Err(SessionCodecError::EventRoute);
                        }
                        SuccessResult::Health(_) => {}
                        SuccessResult::Enabled(enabled) if !enabled.enabled => {
                            if let Some(route) = &mut self.capture_event_route {
                                route.clear_ephemeral();
                            }
                        }
                        SuccessResult::Configuration(_) => {
                            if let Some(route) = &mut self.capture_event_route {
                                route.clear_ephemeral();
                            }
                        }
                        SuccessResult::Paste(result) => {
                            if let Some(operation) =
                                self.pending_paste_operations.remove(&correlation)
                                && matches!(result, PasteResult::Waiting { .. })
                            {
                                let route = self
                                    .capture_event_route
                                    .as_mut()
                                    .ok_or(SessionCodecError::EventRoute)?;
                                route.wait_for_paste(operation)?;
                            }
                        }
                        _ => {}
                    }
                } else {
                    self.pending_paste_operations.remove(&correlation);
                }
                GatewayMessage::Response {
                    correlation_sequence: correlation,
                    response,
                }
            }
            OwnerMessage::Event(event) => {
                if matches!(event, Event::RegisteredObservation(_))
                    && !self.supports_feature(crate::REGISTERED_INPUT_OBSERVABILITY_V1)
                {
                    return Err(SessionCodecError::EventRoute);
                }
                if let Event::HealthChanged(health) = &event
                    && health.owner_instance_id != self.owner_instance_id
                {
                    return Err(SessionCodecError::EventRoute);
                }
                if is_capture_scoped_event(&event)
                    || self
                        .capture_event_route
                        .as_ref()
                        .is_some_and(CaptureEventRoute::is_revoked)
                {
                    self.capture_event_route
                        .as_mut()
                        .ok_or(SessionCodecError::EventRoute)?
                        .accept_event(&event, Some(self.owner_instance_id))?;
                }
                GatewayMessage::Event(event)
            }
            OwnerMessage::PredecessorTerminal(event) => {
                if matches!(event, PredecessorTerminalEvent::LeaseRevoked { .. }) {
                    self.capture_event_route
                        .as_mut()
                        .ok_or(SessionCodecError::EventRoute)?
                        .revoke();
                }
                GatewayMessage::PredecessorTerminal(event)
            }
        };
        Ok(result)
    }

    #[must_use]
    pub fn outstanding_requests(&self) -> usize {
        self.correlations.len()
    }
}
