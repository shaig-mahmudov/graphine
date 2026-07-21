package dev.graphine.fixture.web;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping(path = {"/events", "/activities"})
public final class AliasController {
    @GetMapping("/{id}")
    public Event getAlias(long id) { return new Event(id, "alias"); }
}
