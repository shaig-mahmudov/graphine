package dev.graphine.fixture.beans;

import org.springframework.boot.context.properties.ConfigurationProperties;
import org.springframework.stereotype.Component;

@Component
@ConfigurationProperties(prefix = "mail")
public final class MailProperties {
    private String apiKey;
    private String host;

    public String getApiKey() { return apiKey; }
    public void setApiKey(String apiKey) { this.apiKey = apiKey; }
    public String getHost() { return host; }
    public void setHost(String host) { this.host = host; }
}
