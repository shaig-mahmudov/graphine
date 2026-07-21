package dev.graphine.fixture.multi.api;

public interface Greeter {
    default String greet(String name) { return "Hello " + name; }
}
