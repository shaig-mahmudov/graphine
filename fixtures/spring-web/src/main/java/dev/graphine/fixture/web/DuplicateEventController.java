package dev.graphine.fixture.web;

import java.util.List;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/v1/events")
public final class DuplicateEventController {
    @GetMapping
    public List<Event> duplicateList() { return List.of(); }
}
