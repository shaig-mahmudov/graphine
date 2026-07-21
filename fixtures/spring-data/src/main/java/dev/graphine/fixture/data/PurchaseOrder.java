package dev.graphine.fixture.data;

import jakarta.persistence.Entity;
import jakarta.persistence.GeneratedValue;
import jakarta.persistence.Id;
import jakarta.persistence.ManyToOne;

@Entity
public class PurchaseOrder {
    @Id @GeneratedValue private Long id;
    private String status;
    @ManyToOne(optional = false)
    private Customer customer;

    protected PurchaseOrder() {}
    public PurchaseOrder(String status) { this.status = status; }
    public Long getId() { return id; }
    public String getStatus() { return status; }
    public Customer getCustomer() { return customer; }
    void assignTo(Customer customer) { this.customer = customer; }
    public void changeStatus(String status) { this.status = status; }
}
