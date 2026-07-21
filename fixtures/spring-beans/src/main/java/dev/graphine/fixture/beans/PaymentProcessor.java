package dev.graphine.fixture.beans;

public interface PaymentProcessor {
    String charge(int cents);
}
