package dev.graphine.fixture.beans;

import org.springframework.context.annotation.Primary;
import org.springframework.stereotype.Component;

@Primary
@Component("cardProcessor")
public final class CardProcessor implements PaymentProcessor {
    public String charge(int cents) { return "card:" + cents; }
}
