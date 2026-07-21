package dev.graphine.fixture.beans;

import org.springframework.stereotype.Service;

@Service
public final class CheckoutService {
    private final PaymentProcessor processor;

    public CheckoutService(PaymentProcessor processor) { this.processor = processor; }
    public String checkout(int cents) { return processor.charge(cents); }
}
