package dev.graphine.fixture.core;

public final class CustomDefaultFormatter implements DefaultFormatter {
    @Override
    public String format(String value) {
        return DefaultFormatter.super.format(value.trim());
    }
}
