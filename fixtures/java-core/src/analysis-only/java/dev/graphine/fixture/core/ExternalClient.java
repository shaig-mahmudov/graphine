package dev.graphine.fixture.core;

import com.vendor.missing.RemoteGateway;

public final class ExternalClient {
    private final RemoteGateway gateway;

    public ExternalClient(RemoteGateway gateway) {
        this.gateway = gateway;
    }

    public String fetch() {
        return gateway.fetchValue();
    }
}

