package dev.graphine.fixture.web;

import java.util.List;
import java.util.Optional;

public interface EventRepository {
    Event save(Event event);
    Optional<Event> findById(long id);
    List<Event> findAll();
    void deleteById(long id);
}
