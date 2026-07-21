package dev.graphine.fixture.beans;

import org.springframework.beans.factory.annotation.Qualifier;
import org.springframework.stereotype.Service;

@Service
public final class WireCheckoutService {
    private final PaymentProcessor processor;

    public WireCheckoutService(@Qualifier("wireProcessor") PaymentProcessor processor) {
        this.processor = processor;
    }
}
