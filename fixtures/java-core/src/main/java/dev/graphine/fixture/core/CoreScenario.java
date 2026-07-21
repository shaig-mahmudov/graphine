package dev.graphine.fixture.core;

import java.util.List;

public final class CoreScenario {
    public String run() {
        Operations operations = new Operations.Factory().create();
        String combined = operations.combine("graph", "ine");
        String first = operations.first(List.of(combined));
        return new DecoratingProcessor().process(first);
    }
}
