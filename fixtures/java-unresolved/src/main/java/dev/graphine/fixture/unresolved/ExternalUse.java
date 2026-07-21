package dev.graphine.fixture.unresolved;

import com.vendor.remote.RemoteClient;

public final class ExternalUse {
    private final RemoteClient client;

    public ExternalUse(RemoteClient client) {
        this.client = client;
    }

    public String send(String value) {
        return client.send(value);
    }
}
