package dev.graphine.fixture.beans;

// Analysis-only scenario: if primary metadata is ignored, this injection has multiple candidates.
public final class AmbiguousProcessorConsumer {
    private final PaymentProcessor processor;

    public AmbiguousProcessorConsumer(PaymentProcessor processor) {
        this.processor = processor;
    }
}

