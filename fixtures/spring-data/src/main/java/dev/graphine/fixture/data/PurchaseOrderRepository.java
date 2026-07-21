package dev.graphine.fixture.data;

import java.util.List;
import org.springframework.data.jpa.repository.JpaRepository;
import org.springframework.data.jpa.repository.Query;
import org.springframework.data.repository.query.Param;

public interface PurchaseOrderRepository extends JpaRepository<PurchaseOrder, Long> {
    List<PurchaseOrder> findByStatusOrderByIdAsc(String status);

    @Query("select o from PurchaseOrder o join fetch o.customer where o.customer.email = :email")
    List<PurchaseOrder> findDetailedByCustomerEmail(@Param("email") String email);
}
