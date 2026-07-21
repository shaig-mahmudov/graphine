package dev.graphine.fixture.multi.service;

import dev.graphine.fixture.multi.api.Greeter;

public final class DefaultGreeter implements Greeter {
    @Override
    public String greet(String name) {
        return Greeter.super.greet(name.trim());
    }
}
