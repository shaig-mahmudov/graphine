package dev.graphine.fixture.core;

public final class DecoratingProcessor extends BaseProcessor {
    @Override
    public String process(String value) {
        return "[" + super.process(value) + "]";
    }
}
