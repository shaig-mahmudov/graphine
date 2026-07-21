package dev.graphine.fixture.core;

import java.util.List;
import java.util.function.Function;

@FixtureTag("language-cases")
public final class Operations {
    private int invocationCount;

    public String combine(String left, String right) {
        invocationCount++;
        return left + right;
    }

    public int combine(int left, int right) {
        invocationCount++;
        return left + right;
    }

    public <T> T first(List<T> values) {
        return values.get(0);
    }

    public List<String> transform(List<String> values) {
        Function<String, String> trim = value -> value.trim();
        return values.stream().map(trim).map(String::toUpperCase).toList();
    }

    public int invocationCount() {
        return invocationCount;
    }

    public record Pair(String left, String right) {}

    public static final class Factory {
        public Operations create() {
            return new Operations();
        }
    }

    public String repeatTrim(String value) {
        String first = value.trim();
        String second = value.trim();
        return first + second + value.trim();
    }
}
