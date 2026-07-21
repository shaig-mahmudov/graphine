package dev.graphine.fixture.web;

import java.util.List;
import java.util.concurrent.atomic.AtomicLong;
import org.springframework.context.ApplicationEventPublisher;
import org.springframework.stereotype.Service;

@Service
public final class EventService {
    private final EventRepository repository;
    private final ApplicationEventPublisher publisher;
    private final AtomicLong ids = new AtomicLong();

    public EventService(EventRepository repository, ApplicationEventPublisher publisher) {
        this.repository = repository;
        this.publisher = publisher;
    }

    public List<Event> list() { return repository.findAll(); }
    public Event get(long id) { return repository.findById(id).orElseThrow(); }

    public Event create(CreateEventRequest request) {
        Event saved = repository.save(new Event(ids.incrementAndGet(), request.name()));
        publisher.publishEvent(new EventCreated(saved.id()));
        return saved;
    }

    public void delete(long id) { repository.deleteById(id); }
}
