package dev.graphine.fixture.core;

public class BaseProcessor {
    protected String normalize(String value) {
        return value.trim();
    }

    public String process(String value) {
        return normalize(value);
    }
}
