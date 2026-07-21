package dev.graphine.fixture.data;

import jakarta.persistence.CascadeType;
import jakarta.persistence.Entity;
import jakarta.persistence.GeneratedValue;
import jakarta.persistence.Id;
import jakarta.persistence.OneToMany;
import java.util.ArrayList;
import java.util.List;

@Entity
public class Customer {
    @Id @GeneratedValue private Long id;
    private String email;
    @OneToMany(mappedBy = "customer", cascade = CascadeType.ALL)
    private final List<PurchaseOrder> orders = new ArrayList<>();

    protected Customer() {}
    public Customer(String email) { this.email = email; }
    public Long getId() { return id; }
    public String getEmail() { return email; }
    public List<PurchaseOrder> getOrders() { return orders; }
    public void addOrder(PurchaseOrder order) { orders.add(order); order.assignTo(this); }
}
