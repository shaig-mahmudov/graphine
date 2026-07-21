package dev.graphine.fixture.web;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import org.springframework.stereotype.Repository;

@Repository
public final class InMemoryEventRepository implements EventRepository {
    private final Map<Long, Event> events = new LinkedHashMap<>();

    public Event save(Event event) { events.put(event.id(), event); return event; }
    public Optional<Event> findById(long id) { return Optional.ofNullable(events.get(id)); }
    public List<Event> findAll() { return List.copyOf(events.values()); }
    public void deleteById(long id) { events.remove(id); }
}
