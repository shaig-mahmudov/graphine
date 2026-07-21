package dev.graphine.fixture.beans;

import org.springframework.context.annotation.Profile;
import org.springframework.stereotype.Component;

@Profile("wire")
@Component("wireProcessor")
public final class WireProcessor implements PaymentProcessor {
    public String charge(int cents) { return "wire:" + cents; }
}
