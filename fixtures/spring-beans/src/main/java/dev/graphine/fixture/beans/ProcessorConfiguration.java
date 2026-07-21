package dev.graphine.fixture.beans;

import org.springframework.boot.autoconfigure.condition.ConditionalOnProperty;
import org.springframework.context.annotation.Bean;
import org.springframework.context.annotation.Configuration;

@Configuration
public class ProcessorConfiguration {
    @Bean("auditLabel")
    public String auditLabel() { return "payments"; }

    @Bean("conditionalProcessor")
    @ConditionalOnProperty(name = "payments.conditional", havingValue = "true")
    public PaymentProcessor conditionalProcessor() {
        return cents -> "conditional:" + cents;
    }
}
