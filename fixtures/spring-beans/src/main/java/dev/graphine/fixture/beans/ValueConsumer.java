package dev.graphine.fixture.beans;

import org.springframework.beans.factory.annotation.Value;
import org.springframework.stereotype.Component;

@Component
public final class ValueConsumer {
    @Value("${payments.token:unset}")
    private String token;
}
