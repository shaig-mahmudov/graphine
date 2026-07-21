package dev.graphine.fixture.data;

import java.util.List;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

@Service
public class OrderService {
    private final CustomerRepository customers;
    private final PurchaseOrderRepository orders;

    public OrderService(CustomerRepository customers, PurchaseOrderRepository orders) {
        this.customers = customers;
        this.orders = orders;
    }

    @Transactional
    public PurchaseOrder create(String email) {
        Customer customer = customers.findByEmailIgnoreCase(email)
            .orElseGet(() -> customers.save(new Customer(email)));
        PurchaseOrder order = new PurchaseOrder("NEW");
        customer.addOrder(order);
        return orders.save(order);
    }

    @Transactional(readOnly = true)
    public List<PurchaseOrder> openOrders() {
        return orders.findByStatusOrderByIdAsc("NEW");
    }

    @Transactional
    public void complete(long id) {
        orders.findById(id).orElseThrow().changeStatus("COMPLETE");
    }
}
