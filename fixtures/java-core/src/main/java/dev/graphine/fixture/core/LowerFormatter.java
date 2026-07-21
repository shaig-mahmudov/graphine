package dev.graphine.fixture.core;

public final class LowerFormatter implements Formatter {
    @Override
    public String format(String value) {
        return value.toLowerCase();
    }
}
