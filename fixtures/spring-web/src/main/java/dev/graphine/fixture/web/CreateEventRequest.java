package dev.graphine.fixture.web;

import jakarta.validation.constraints.NotBlank;

public record CreateEventRequest(@NotBlank String name) {}
