package dev.graphine.fixture.core;

public final class UpperFormatter implements Formatter {
    @Override
    public String format(String value) {
        return value.toUpperCase();
    }
}
