package dev.graphine.fixture.core;

public interface DefaultFormatter extends Formatter {
    default String format(String value) {
        return "default:" + value;
    }
}
