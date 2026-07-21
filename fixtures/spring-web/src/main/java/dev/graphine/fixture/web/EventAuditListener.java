package dev.graphine.fixture.web;

import org.springframework.context.event.EventListener;
import org.springframework.stereotype.Component;

@Component
public final class EventAuditListener {
    @EventListener
    public void record(EventCreated event) {
        // Deterministic no-op listener used by structural benchmarks.
    }
}

