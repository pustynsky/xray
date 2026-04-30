//! SQL parser tests — regex-based T-SQL definition extraction.

use super::*;
use super::parser_sql::parse_sql_definitions;

// ─── Test 1: CREATE PROCEDURE ──────────────────────────────────────

#[test]
fn test_sql_create_procedure() {
    let source = r#"
CREATE PROCEDURE [Sales].[usp_CreateOrder]
    @CustomerId INT,
    @ProductId INT,
    @Quantity SMALLINT,
    @Price DECIMAL(18,2)
AS
BEGIN
    INSERT INTO [Sales].[Orders] ([CustomerId], [ProductId], [Quantity], [Price])
    VALUES (@CustomerId, @ProductId, @Quantity, @Price)
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let proc_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::StoredProcedure).collect();
    assert_eq!(proc_defs.len(), 1, "Expected 1 stored procedure");
    assert_eq!(proc_defs[0].name, "usp_CreateOrder");
    assert!(proc_defs[0].signature.is_some());
    let sig = proc_defs[0].signature.as_ref().unwrap();
    assert!(sig.contains("usp_CreateOrder"), "Signature should contain proc name, got: {}", sig);
}

// ─── Test 2: CREATE TABLE with columns ─────────────────────────────

#[test]
fn test_sql_create_table_with_columns() {
    let source = r#"
CREATE TABLE [dbo].[Orders]
(
    [OrderId] BIGINT IDENTITY(1,1) NOT NULL,
    [CustomerId] INT NOT NULL,
    [ProductName] NVARCHAR(200) NOT NULL,
    [Quantity] SMALLINT NOT NULL,
    [TotalPrice] DECIMAL(18,2) NOT NULL,
    [CreatedDate] DATETIME2 NOT NULL DEFAULT GETUTCDATE(),
    CONSTRAINT [PK_Orders] PRIMARY KEY CLUSTERED ([OrderId] ASC)
)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let table_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Table).collect();
    assert_eq!(table_defs.len(), 1, "Expected 1 table");
    assert_eq!(table_defs[0].name, "Orders");
    assert!(table_defs[0].modifiers.contains(&"primaryKey".to_string()),
        "Table with PK should have primaryKey modifier");

    let col_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Column).collect();
    assert!(col_defs.len() >= 5, "Expected at least 5 column definitions, got {}", col_defs.len());

    // All columns should have parent = "Orders"
    for col in &col_defs {
        assert_eq!(col.parent, Some("Orders".to_string()),
            "Column '{}' should have parent 'Orders'", col.name);
    }

    // Check specific columns exist
    let col_names: Vec<&str> = col_defs.iter().map(|d| d.name.as_str()).collect();
    assert!(col_names.contains(&"OrderId"), "Expected OrderId column, got: {:?}", col_names);
    assert!(col_names.contains(&"CustomerId"), "Expected CustomerId column");
    assert!(col_names.contains(&"ProductName"), "Expected ProductName column");
}

// ─── Test 3: CREATE TABLE with FK constraints ──────────────────────

#[test]
fn test_sql_create_table_with_fk() {
    let source = r#"
CREATE TABLE [dbo].[OrderItems]
(
    [ItemId] BIGINT IDENTITY(1,1) NOT NULL,
    [OrderId] BIGINT NOT NULL,
    [ProductId] INT NOT NULL,
    [WarehouseId] INT NOT NULL,
    CONSTRAINT [PK_OrderItems] PRIMARY KEY CLUSTERED ([ItemId] ASC),
    CONSTRAINT [FK_OrderItems_Orders] FOREIGN KEY ([OrderId]) REFERENCES [dbo].[Orders] ([OrderId]),
    CONSTRAINT [FK_OrderItems_Products] FOREIGN KEY ([ProductId]) REFERENCES [dbo].[Products] ([ProductId]),
    CONSTRAINT [FK_OrderItems_Warehouses] FOREIGN KEY ([WarehouseId]) REFERENCES [Inventory].[Warehouses] ([WarehouseId])
)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let table_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Table).collect();
    assert_eq!(table_defs.len(), 1);
    assert_eq!(table_defs[0].name, "OrderItems");

    // Check FK references in base_types
    let base_types = &table_defs[0].base_types;
    assert!(base_types.contains(&"Orders".to_string()),
        "base_types should contain 'Orders', got: {:?}", base_types);
    assert!(base_types.contains(&"Products".to_string()),
        "base_types should contain 'Products', got: {:?}", base_types);
    assert!(base_types.contains(&"Warehouses".to_string()),
        "base_types should contain 'Warehouses', got: {:?}", base_types);
}

// ─── Test 4: CREATE INDEX ──────────────────────────────────────────

#[test]
fn test_sql_create_index() {
    let source = r#"
CREATE NONCLUSTERED INDEX [IX_Orders_CustomerId] ON [dbo].[Orders]
(
    [CustomerId] ASC
)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let idx_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::SqlIndex).collect();
    assert_eq!(idx_defs.len(), 1, "Expected 1 index");
    assert_eq!(idx_defs[0].name, "IX_Orders_CustomerId");
    assert_eq!(idx_defs[0].parent, Some("Orders".to_string()),
        "Index parent should be 'Orders'");
}

// ─── Test 5: GO-separated objects ──────────────────────────────────

#[test]
fn test_sql_go_separated_objects() {
    let source = r#"CREATE TABLE [dbo].[Products]
(
    [ProductId] INT IDENTITY(1,1) NOT NULL,
    [Name] NVARCHAR(200) NOT NULL,
    CONSTRAINT [PK_Products] PRIMARY KEY CLUSTERED ([ProductId] ASC)
)
GO
CREATE NONCLUSTERED INDEX [IX_Products_Name] ON [dbo].[Products]
(
    [Name] ASC
)
GO
CREATE UNIQUE NONCLUSTERED INDEX [UX_Products_Code] ON [dbo].[Products]
(
    [ProductId] ASC
)
GO
CREATE NONCLUSTERED INDEX [IX_Products_Category] ON [dbo].[Products]
(
    [Name] ASC
)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let table_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Table).collect();
    assert_eq!(table_defs.len(), 1, "Expected 1 table");
    assert_eq!(table_defs[0].name, "Products");

    let idx_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::SqlIndex).collect();
    assert_eq!(idx_defs.len(), 3, "Expected 3 indexes");

    // All indexes should have parent = "Products"
    for idx in &idx_defs {
        assert_eq!(idx.parent, Some("Products".to_string()),
            "Index '{}' should have parent 'Products'", idx.name);
    }

    // Verify line ranges don't overlap with GO boundaries
    // Table starts at line 1, first GO at line 7
    assert_eq!(table_defs[0].line_start, 1);
    assert!(table_defs[0].line_end <= 7,
        "Table line_end should be before first GO, got {}", table_defs[0].line_end);

    // First index after GO (line 8)
    assert!(idx_defs[0].line_start >= 8,
        "First index should start at or after line 8, got {}", idx_defs[0].line_start);
}

// ─── Test 6: CREATE VIEW ───────────────────────────────────────────

#[test]
fn test_sql_create_view() {
    let source = r#"
CREATE VIEW [Reports].[vw_OrderSummary]
AS
SELECT
    o.[OrderId],
    o.[CustomerId],
    c.[CustomerName],
    o.[TotalPrice]
FROM [dbo].[Orders] o
JOIN [dbo].[Customers] c ON o.[CustomerId] = c.[CustomerId]
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let view_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::View).collect();
    assert_eq!(view_defs.len(), 1, "Expected 1 view");
    assert_eq!(view_defs[0].name, "vw_OrderSummary");
    let sig = view_defs[0].signature.as_ref().unwrap();
    assert!(sig.contains("VIEW"), "Signature should contain VIEW, got: {}", sig);
}

// ─── Test 7: CREATE FUNCTION ───────────────────────────────────────

#[test]
fn test_sql_create_function() {
    let source = r#"
CREATE FUNCTION [dbo].[udf_CalculateDiscount]
(
    @TotalAmount DECIMAL(18,2),
    @DiscountRate FLOAT
)
RETURNS DECIMAL(18,2)
AS
BEGIN
    RETURN @TotalAmount * @DiscountRate
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let func_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::SqlFunction).collect();
    assert_eq!(func_defs.len(), 1, "Expected 1 function");
    assert_eq!(func_defs[0].name, "udf_CalculateDiscount");
}

// ─── Test 8: CREATE TYPE ───────────────────────────────────────────

#[test]
fn test_sql_create_type() {
    let source = r#"
CREATE TYPE [dbo].[OrderItemTableType] AS TABLE
(
    [ProductId] INT NOT NULL,
    [Quantity] SMALLINT NOT NULL,
    [Price] DECIMAL(18,2) NOT NULL
)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let type_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::UserDefinedType).collect();
    assert_eq!(type_defs.len(), 1, "Expected 1 user-defined type");
    assert_eq!(type_defs[0].name, "OrderItemTableType");
}

// ─── Test 9: CREATE OR ALTER ───────────────────────────────────────

#[test]
fn test_sql_create_or_alter() {
    let source = r#"
CREATE OR ALTER PROCEDURE [Sales].[usp_UpdateOrder]
    @OrderId BIGINT,
    @NewQuantity SMALLINT
AS
BEGIN
    UPDATE [dbo].[Orders]
    SET [Quantity] = @NewQuantity
    WHERE [OrderId] = @OrderId
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let proc_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::StoredProcedure).collect();
    assert_eq!(proc_defs.len(), 1, "Expected 1 stored procedure");
    assert_eq!(proc_defs[0].name, "usp_UpdateOrder");
}

// ─── Test 10: Call sites — EXEC ────────────────────────────────────

#[test]
fn test_sql_call_sites_exec() {
    let source = r#"
CREATE PROCEDURE [Sales].[usp_ProcessOrder]
    @OrderId BIGINT
AS
BEGIN
    EXEC [Sales].[usp_ValidateOrder] @OrderId
    EXEC [Inventory].[usp_ReserveStock] @OrderId
    EXECUTE [Notifications].[usp_SendConfirmation] @OrderId
END
"#;
    let (defs, call_sites, _) = parse_sql_definitions(source, 0);

    assert_eq!(defs.len(), 1, "Expected 1 procedure");
    assert_eq!(defs[0].name, "usp_ProcessOrder");

    assert!(!call_sites.is_empty(), "Expected call sites");
    let (def_idx, calls) = &call_sites[0];
    assert_eq!(*def_idx, 0);

    let call_names: Vec<&str> = calls.iter().map(|c| c.method_name.as_str()).collect();
    assert!(call_names.contains(&"usp_ValidateOrder"),
        "Expected EXEC call to usp_ValidateOrder, got: {:?}", call_names);
    assert!(call_names.contains(&"usp_ReserveStock"),
        "Expected EXEC call to usp_ReserveStock, got: {:?}", call_names);
    assert!(call_names.contains(&"usp_SendConfirmation"),
        "Expected EXECUTE call to usp_SendConfirmation, got: {:?}", call_names);

    // Check receiver_type contains schema
    let validate = calls.iter().find(|c| c.method_name == "usp_ValidateOrder").unwrap();
    assert_eq!(validate.receiver_type.as_deref(), Some("Sales"),
        "EXEC receiver_type should be schema 'Sales'");
}

// ─── Test 11: Call sites — FROM/JOIN ───────────────────────────────

#[test]
fn test_sql_call_sites_from_join() {
    let source = r#"
CREATE PROCEDURE [Reports].[usp_GetOrderReport]
    @CustomerId INT
AS
BEGIN
    SELECT o.*, c.[CustomerName]
    FROM [dbo].[Orders] o
    INNER JOIN [dbo].[Customers] c ON o.[CustomerId] = c.[CustomerId]
    LEFT JOIN [Inventory].[Warehouses] w ON o.[WarehouseId] = w.[WarehouseId]
    WHERE o.[CustomerId] = @CustomerId
END
"#;
    let (defs, call_sites, _) = parse_sql_definitions(source, 0);

    assert_eq!(defs.len(), 1);
    assert!(!call_sites.is_empty(), "Expected call sites");
    let (_, calls) = &call_sites[0];

    let call_names: Vec<&str> = calls.iter().map(|c| c.method_name.as_str()).collect();
    assert!(call_names.contains(&"Orders"),
        "Expected FROM call to Orders, got: {:?}", call_names);
    assert!(call_names.contains(&"Customers"),
        "Expected JOIN call to Customers, got: {:?}", call_names);
    assert!(call_names.contains(&"Warehouses"),
        "Expected JOIN call to Warehouses, got: {:?}", call_names);
}

// ─── Test 12: Call sites — INSERT INTO / UPDATE ────────────────────

#[test]
fn test_sql_call_sites_insert_update() {
    let source = r#"
CREATE PROCEDURE [Sales].[usp_CreateAndUpdateOrder]
    @CustomerId INT,
    @ProductId INT
AS
BEGIN
    INSERT INTO [dbo].[Orders] ([CustomerId], [ProductId])
    VALUES (@CustomerId, @ProductId)

    UPDATE [dbo].[OrderStats]
    SET [OrderCount] = [OrderCount] + 1
    WHERE [CustomerId] = @CustomerId

    DELETE FROM [dbo].[TempOrders]
    WHERE [CustomerId] = @CustomerId
END
"#;
    let (defs, call_sites, _) = parse_sql_definitions(source, 0);

    assert_eq!(defs.len(), 1);
    assert!(!call_sites.is_empty(), "Expected call sites");
    let (_, calls) = &call_sites[0];

    let call_names: Vec<&str> = calls.iter().map(|c| c.method_name.as_str()).collect();
    assert!(call_names.contains(&"Orders"),
        "Expected INSERT INTO call to Orders, got: {:?}", call_names);
    assert!(call_names.contains(&"OrderStats"),
        "Expected UPDATE call to OrderStats, got: {:?}", call_names);
    assert!(call_names.contains(&"TempOrders"),
        "Expected DELETE FROM call to TempOrders, got: {:?}", call_names);
}

// ─── Test 13: Real-world table with indexes and FKs ────────────────

#[test]
fn test_sql_real_world_table() {
    let source = r#"CREATE TABLE [dbo].[OrderItems]
(
    [ItemId] BIGINT IDENTITY(1,1) NOT NULL,
    [OrderId] BIGINT NOT NULL,
    [ProductId] INT NOT NULL,
    [Quantity] SMALLINT NOT NULL DEFAULT(1),
    [UnitPrice] DECIMAL(18,2) NOT NULL,
    [DiscountAmount] DECIMAL(18,2) NULL,
    [TaxRate] FLOAT NOT NULL DEFAULT(0.0),
    [StatusCode] TINYINT NOT NULL DEFAULT(0),
    [Notes] NVARCHAR(500) NULL,
    [CreatedDate] DATETIME2 NOT NULL DEFAULT GETUTCDATE(),
    [ModifiedDate] DATETIME2 NULL,
    [CreatedBy] UNIQUEIDENTIFIER NOT NULL,
    CONSTRAINT [PK_OrderItems] PRIMARY KEY CLUSTERED ([ItemId] ASC),
    CONSTRAINT [FK_OrderItems_Orders] FOREIGN KEY ([OrderId]) REFERENCES [dbo].[Orders] ([OrderId]),
    CONSTRAINT [FK_OrderItems_Products] FOREIGN KEY ([ProductId]) REFERENCES [Catalog].[Products] ([ProductId]),
    CONSTRAINT [FK_OrderItems_Users] FOREIGN KEY ([CreatedBy]) REFERENCES [dbo].[Users] ([UserId])
)
GO
CREATE UNIQUE NONCLUSTERED INDEX [UX_OrderItems_OrderProduct] ON [dbo].[OrderItems]
(
    [OrderId] ASC,
    [ProductId] ASC
)
GO
CREATE NONCLUSTERED INDEX [IX_OrderItems_ProductId] ON [dbo].[OrderItems]
(
    [ProductId] ASC
)
GO
CREATE NONCLUSTERED INDEX [IX_OrderItems_CreatedDate] ON [dbo].[OrderItems]
(
    [CreatedDate] ASC
)
INCLUDE ([OrderId], [StatusCode])
GO
CREATE NONCLUSTERED INDEX [IX_OrderItems_StatusCode] ON [dbo].[OrderItems]
(
    [StatusCode] ASC
)
WHERE [StatusCode] <> 0
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    // 1 table
    let table_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Table).collect();
    assert_eq!(table_defs.len(), 1, "Expected 1 table");
    assert_eq!(table_defs[0].name, "OrderItems");

    // Columns
    let col_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Column).collect();
    assert!(col_defs.len() >= 10, "Expected at least 10 columns, got {}", col_defs.len());

    // FK references
    let base_types = &table_defs[0].base_types;
    assert!(base_types.contains(&"Orders".to_string()), "FK to Orders");
    assert!(base_types.contains(&"Products".to_string()), "FK to Products");
    assert!(base_types.contains(&"Users".to_string()), "FK to Users");

    // PK
    assert!(table_defs[0].modifiers.contains(&"primaryKey".to_string()), "Has PK");

    // 4 indexes
    let idx_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::SqlIndex).collect();
    assert_eq!(idx_defs.len(), 4, "Expected 4 indexes, got {}", idx_defs.len());

    // All indexes should reference OrderItems as parent
    for idx in &idx_defs {
        assert_eq!(idx.parent, Some("OrderItems".to_string()),
            "Index '{}' should reference table 'OrderItems'", idx.name);
    }

    let idx_names: Vec<&str> = idx_defs.iter().map(|d| d.name.as_str()).collect();
    assert!(idx_names.contains(&"UX_OrderItems_OrderProduct"), "Expected UX_OrderItems_OrderProduct");
    assert!(idx_names.contains(&"IX_OrderItems_ProductId"), "Expected IX_OrderItems_ProductId");
    assert!(idx_names.contains(&"IX_OrderItems_CreatedDate"), "Expected IX_OrderItems_CreatedDate");
    assert!(idx_names.contains(&"IX_OrderItems_StatusCode"), "Expected IX_OrderItems_StatusCode");
}

// ─── Test 14: Real-world procedure with call sites ─────────────────

#[test]
fn test_sql_real_world_procedure() {
    let source = r#"
CREATE PROCEDURE [Sales].[usp_CreateOrderWithValidation]
    @CustomerId INT,
    @ProductId INT,
    @Quantity SMALLINT,
    @Price DECIMAL(18,2),
    @RequestedBy UNIQUEIDENTIFIER
AS
BEGIN
    SET NOCOUNT ON;

    -- Validate customer access
    EXEC [Security].[usp_ValidateCustomerAccess] @CustomerId, @RequestedBy

    -- Check product availability
    EXEC [Inventory].[usp_CheckProductAvailability] @ProductId, @Quantity

    -- Insert the order
    INSERT INTO [dbo].[Orders] ([CustomerId], [ProductId], [Quantity], [Price], [CreatedBy])
    VALUES (@CustomerId, @ProductId, @Quantity, @Price, @RequestedBy)

    DECLARE @NewOrderId BIGINT = SCOPE_IDENTITY()

    -- Update customer stats
    UPDATE [dbo].[CustomerStats]
    SET [LastOrderDate] = GETUTCDATE(), [TotalOrders] = [TotalOrders] + 1
    WHERE [CustomerId] = @CustomerId

    -- Get order details (join with lookup tables)
    SELECT o.*, p.[ProductName], c.[CustomerName]
    FROM [dbo].[Orders] o
    JOIN [Catalog].[Products] p ON o.[ProductId] = p.[ProductId]
    JOIN [dbo].[Customers] c ON o.[CustomerId] = c.[CustomerId]
    WHERE o.[OrderId] = @NewOrderId
END
"#;
    let (defs, call_sites, _) = parse_sql_definitions(source, 0);

    assert_eq!(defs.len(), 1, "Expected 1 procedure");
    assert_eq!(defs[0].name, "usp_CreateOrderWithValidation");
    assert_eq!(defs[0].kind, DefinitionKind::StoredProcedure);

    assert!(!call_sites.is_empty(), "Expected call sites");
    let (_, calls) = &call_sites[0];

    let call_names: Vec<&str> = calls.iter().map(|c| c.method_name.as_str()).collect();

    // EXEC calls
    assert!(call_names.contains(&"usp_ValidateCustomerAccess"),
        "Expected EXEC usp_ValidateCustomerAccess, got: {:?}", call_names);
    assert!(call_names.contains(&"usp_CheckProductAvailability"),
        "Expected EXEC usp_CheckProductAvailability, got: {:?}", call_names);

    // Table references (deduplicated)
    assert!(call_names.contains(&"Orders"),
        "Expected Orders table reference, got: {:?}", call_names);
    assert!(call_names.contains(&"CustomerStats"),
        "Expected CustomerStats table reference, got: {:?}", call_names);
    assert!(call_names.contains(&"Products"),
        "Expected Products table reference, got: {:?}", call_names);
    assert!(call_names.contains(&"Customers"),
        "Expected Customers table reference, got: {:?}", call_names);
}

// ─── Test 15: Schema stripped from name ────────────────────────────

#[test]
fn test_sql_schema_stripped() {
    let source = r#"
CREATE TABLE [dbo].[OrderItems]
(
    [ItemId] BIGINT NOT NULL
)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let table = defs.iter().find(|d| d.kind == DefinitionKind::Table).unwrap();
    assert_eq!(table.name, "OrderItems",
        "Name should be 'OrderItems' without schema prefix, got: '{}'", table.name);
    // Schema should be in the signature, not the name
    let sig = table.signature.as_ref().unwrap();
    assert!(sig.contains("dbo"), "Signature should contain schema 'dbo', got: {}", sig);
}

// ─── Test 16: Brackets stripped from names ─────────────────────────

#[test]
fn test_sql_brackets_stripped() {
    let source = r#"
CREATE UNIQUE NONCLUSTERED INDEX [UX_ProductCode] ON [dbo].[Products]
(
    [ProductCode] ASC
)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let idx = defs.iter().find(|d| d.kind == DefinitionKind::SqlIndex).unwrap();
    assert_eq!(idx.name, "UX_ProductCode",
        "Index name should have brackets stripped, got: '{}'", idx.name);
    assert_eq!(idx.parent, Some("Products".to_string()),
        "Parent table name should have brackets stripped");
}

// ─── Test 17: Empty file ───────────────────────────────────────────

#[test]
fn test_sql_empty_file() {
    let (defs, calls, stats) = parse_sql_definitions("", 0);
    assert!(defs.is_empty(), "Empty file should produce 0 definitions");
    assert!(calls.is_empty(), "Empty file should produce 0 call sites");
    assert!(stats.is_empty(), "Empty file should produce 0 code stats");
}

// ─── Test 18: Comments-only file ───────────────────────────────────

#[test]
fn test_sql_comments_only() {
    let source = r#"
-- This is a comment
-- Another comment
-- No actual SQL statements here
"#;
    let (defs, calls, stats) = parse_sql_definitions(source, 0);
    assert!(defs.is_empty(), "Comments-only file should produce 0 definitions");
    assert!(calls.is_empty(), "Comments-only file should produce 0 call sites");
    assert!(stats.is_empty(), "Comments-only file should produce 0 code stats");
}

// ─── Additional edge case tests ────────────────────────────────────

#[test]
fn test_sql_create_proc_shorthand() {
    // CREATE PROC (shorthand for PROCEDURE)
    let source = r#"
CREATE PROC [dbo].[usp_QuickLookup]
    @Id INT
AS
BEGIN
    SELECT * FROM [dbo].[Items] WHERE [Id] = @Id
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let proc_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::StoredProcedure).collect();
    assert_eq!(proc_defs.len(), 1, "Expected 1 stored procedure for CREATE PROC");
    assert_eq!(proc_defs[0].name, "usp_QuickLookup");
}

#[test]
fn test_sql_unqualified_names() {
    // Names without schema qualification
    let source = r#"
CREATE TABLE SimpleTable
(
    Id INT NOT NULL,
    Name NVARCHAR(100) NOT NULL
)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let table = defs.iter().find(|d| d.kind == DefinitionKind::Table).unwrap();
    assert_eq!(table.name, "SimpleTable");
}

#[test]
fn test_sql_call_sites_deduplication() {
    // Same table referenced multiple times should result in one call site
    let source = r#"
CREATE PROCEDURE [dbo].[usp_OrderReport]
AS
BEGIN
    SELECT * FROM [dbo].[Orders]
    INSERT INTO [dbo].[Orders] ([Col]) VALUES (1)
    UPDATE [dbo].[Orders] SET [Col] = 2
END
"#;
    let (defs, call_sites, _) = parse_sql_definitions(source, 0);

    assert_eq!(defs.len(), 1);
    assert!(!call_sites.is_empty());
    let (_, calls) = &call_sites[0];

    // Orders should appear only once (deduplicated)
    let orders_calls: Vec<_> = calls.iter().filter(|c| c.method_name == "Orders").collect();
    assert_eq!(orders_calls.len(), 1,
        "Duplicate references to same table should be deduplicated, got: {}",
        orders_calls.len());
}

#[test]
fn test_sql_go_case_insensitive() {
    // GO can be lowercase, uppercase, or mixed case
    let source = r#"CREATE TABLE [dbo].[TableA]
(
    [Id] INT NOT NULL
)
go
CREATE TABLE [dbo].[TableB]
(
    [Id] INT NOT NULL
)
Go
CREATE TABLE [dbo].[TableC]
(
    [Id] INT NOT NULL
)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let table_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::Table).collect();
    assert_eq!(table_defs.len(), 3, "Expected 3 tables separated by GO variants");

    let names: Vec<&str> = table_defs.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"TableA"));
    assert!(names.contains(&"TableB"));
    assert!(names.contains(&"TableC"));
}

#[test]
fn test_sql_procedure_parameters_in_signature() {
    let source = r#"
CREATE PROCEDURE [dbo].[usp_SearchOrders]
    @CustomerId INT,
    @StartDate DATETIME2,
    @EndDate DATETIME2,
    @StatusCode TINYINT = NULL,
    @MaxResults INT = 100
AS
BEGIN
    SELECT * FROM [dbo].[Orders]
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let proc = defs.iter().find(|d| d.kind == DefinitionKind::StoredProcedure).unwrap();
    let sig = proc.signature.as_ref().unwrap();

    // Signature should contain parameter names
    assert!(sig.contains("@CustomerId"), "Signature should include @CustomerId, got: {}", sig);
    assert!(sig.contains("@StartDate"), "Signature should include @StartDate, got: {}", sig);
}

#[test]
fn test_sql_procedure_signature_uses_header_params_only() {
    let source = r#"
CREATE PROCEDURE [Modifiers].[usp_Example]
     @CallingWorkspaceId AS BIGINT
    ,@TenantId AS BIGINT
    ,@AccessRequestApprovalContext AS [Modifiers].[udtt_AccessRequestApprovalContext] READONLY
AS
BEGIN
    SELECT @ContentProviderDisplayText = cp.DisplayText,
           @ContentProviderFolderId = cp.FolderId,
           @ContentProviderKey = cp.ProviderKey
    FROM [dbo].[ContentProviders_V0] AS cp

    EXEC Modifiers.nsp_EnsureContentProviderAccess_V40
        @CallingWorkspaceId = -1,
        @CallingTenantId = @TenantId
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let proc = defs.iter().find(|d| d.kind == DefinitionKind::StoredProcedure).unwrap();
    let sig = proc.signature.as_ref().unwrap();

    assert!(
        sig.contains("(@CallingWorkspaceId, @TenantId, @AccessRequestApprovalContext)"),
        "Signature should include only header params, got: {}",
        sig
    );
    assert!(!sig.contains("@ContentProviderFolderId"), "Signature should not include body local values, got: {}", sig);
    assert!(!sig.contains("@ContentProviderKey"), "Signature should not include body local values, got: {}", sig);
    assert!(!sig.contains("@CallingTenantId"), "Signature should not include EXEC call args, got: {}", sig);
}

#[test]
fn test_sql_function_signature_params_stop_before_returns() {
    let source = r#"
CREATE FUNCTION [dbo].[udf_Example]
(
    @InputId INT
    ,@TenantId BIGINT
)
RETURNS INT
AS
BEGIN
    RETURN @TenantId
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let func = defs.iter().find(|d| d.kind == DefinitionKind::SqlFunction).unwrap();
    let sig = func.signature.as_ref().unwrap();

    assert!(
        sig.contains("(@InputId, @TenantId)"),
        "Signature should include function header params, got: {}",
        sig
    );
}

#[test]
fn test_sql_inline_procedure_signature_params_stop_before_body() {
    let source = r#"
CREATE PROCEDURE [dbo].[usp_Inline] @TenantId BIGINT, @ReportId BIGINT AS SELECT @BodyLocal = @TenantId
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let proc = defs.iter().find(|d| d.kind == DefinitionKind::StoredProcedure).unwrap();
    let sig = proc.signature.as_ref().unwrap();

    assert_eq!(
        sig,
        "CREATE PROCEDURE [dbo].[usp_Inline] @TenantId BIGINT, @ReportId BIGINT"
    );
    assert!(!sig.contains("@BodyLocal"), "Signature should not include inline body values, got: {}", sig);
}

#[test]
fn test_sql_inline_function_signature_params_stop_before_returns() {
    let source = r#"
CREATE FUNCTION [dbo].[udf_Inline](@InputId INT, @TenantId BIGINT) RETURNS INT AS BEGIN RETURN @TenantId END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let func = defs.iter().find(|d| d.kind == DefinitionKind::SqlFunction).unwrap();
    let sig = func.signature.as_ref().unwrap();

    assert_eq!(
        sig,
        "CREATE FUNCTION [dbo].[udf_Inline](@InputId INT, @TenantId BIGINT)"
    );
}


#[test]
fn test_sql_inline_procedure_signature_preserves_comment_markers_in_defaults() {
    let source = r#"
CREATE PROCEDURE [dbo].[usp_DefaultMarkers] @Dash VARCHAR(8) = '--', @Slash VARCHAR(8) = '/*', @TenantId INT AS SELECT @TenantId
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let proc = defs.iter().find(|d| d.kind == DefinitionKind::StoredProcedure).unwrap();
    let sig = proc.signature.as_ref().unwrap();

    assert_eq!(
        sig,
        "CREATE PROCEDURE [dbo].[usp_DefaultMarkers] @Dash VARCHAR(8) = '--', @Slash VARCHAR(8) = '/*', @TenantId INT"
    );
}

#[test]
fn test_sql_multiline_function_signature_appends_params_after_open_paren() {
    let source = r#"
CREATE FUNCTION [dbo].[udf_MultilineOpen](
    @InputId INT
    ,@TenantId BIGINT
)
RETURNS INT
AS
BEGIN
    RETURN @TenantId
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let func = defs.iter().find(|d| d.kind == DefinitionKind::SqlFunction).unwrap();
    let sig = func.signature.as_ref().unwrap();

    assert_eq!(
        sig,
        "CREATE FUNCTION [dbo].[udf_MultilineOpen](@InputId, @TenantId)"
    );
}


#[test]
fn test_sql_multiline_procedure_signature_rebuilds_partial_first_line_params() {
    let source = r#"
CREATE PROCEDURE [dbo].[usp_Partial] @FirstParam INT,
    @SecondParam BIGINT
AS
BEGIN
    SELECT 1
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let proc = defs.iter().find(|d| d.kind == DefinitionKind::StoredProcedure).unwrap();
    let sig = proc.signature.as_ref().unwrap();

    assert_eq!(
        sig,
        "CREATE PROCEDURE [dbo].[usp_Partial] (@FirstParam, @SecondParam)"
    );
}

#[test]
fn test_sql_multiline_function_signature_rebuilds_partial_first_line_params() {
    let source = r#"
CREATE FUNCTION [dbo].[udf_Partial](@InputId INT,
    @TenantId BIGINT
)
RETURNS INT
AS
BEGIN
    RETURN @TenantId
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let func = defs.iter().find(|d| d.kind == DefinitionKind::SqlFunction).unwrap();
    let sig = func.signature.as_ref().unwrap();

    assert_eq!(
        sig,
        "CREATE FUNCTION [dbo].[udf_Partial](@InputId, @TenantId)"
    );
}

#[test]
fn test_sql_signature_starts_at_real_create_not_commented_out_procedure() {
    let source = r#"
-- CREATE PROCEDURE [dbo].[usp_Fake]
--     @FakeParam INT
-- AS SELECT @FakeParam
CREATE PROCEDURE [dbo].[usp_CommentSafe]
    @RealParam INT
AS
BEGIN
    SELECT 1
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let proc = defs.iter().find(|d| d.kind == DefinitionKind::StoredProcedure).unwrap();
    let sig = proc.signature.as_ref().unwrap();

    assert_eq!(proc.name, "usp_CommentSafe");
    assert!(sig.starts_with("CREATE PROCEDURE"), "Signature should start at real CREATE, got: {}", sig);
    assert!(sig.contains("@RealParam"), "Signature should include real header param, got: {}", sig);
    assert!(!sig.contains("@FakeParam"), "Signature should not include comment text, got: {}", sig);
}

#[test]
fn test_sql_signature_starts_at_real_create_not_commented_out_function() {
    let source = r#"
-- CREATE FUNCTION [dbo].[udf_Fake](@FakeParam INT) RETURNS INT AS BEGIN RETURN @FakeParam END
CREATE FUNCTION [dbo].[udf_CommentSafe](@RealParam INT) RETURNS INT AS BEGIN RETURN @RealParam END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let func = defs.iter().find(|d| d.kind == DefinitionKind::SqlFunction).unwrap();
    let sig = func.signature.as_ref().unwrap();

    assert_eq!(func.name, "udf_CommentSafe");
    assert!(sig.starts_with("CREATE FUNCTION"), "Signature should start at real CREATE, got: {}", sig);
    assert!(sig.contains("@RealParam"), "Signature should include real header param, got: {}", sig);
    assert!(!sig.contains("@FakeParam"), "Signature should not include comment text, got: {}", sig);
}


#[test]
fn test_sql_dispatch_ignores_block_commented_create() {
    let source = r#"
/*
CREATE TABLE [dbo].[FakeTable]
(
    Id INT
)
*/
CREATE PROCEDURE [dbo].[usp_BlockCommentDispatch]
    @RealParam INT
AS
BEGIN
    SELECT 1
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let proc = defs.iter().find(|d| d.kind == DefinitionKind::StoredProcedure).unwrap();
    let sig = proc.signature.as_ref().unwrap();

    assert_eq!(proc.name, "usp_BlockCommentDispatch");
    assert!(sig.contains("(@RealParam)"), "Signature should include real header param, got: {}", sig);
    assert!(defs.iter().all(|d| d.name != "FakeTable"), "Commented-out CREATE TABLE should not be parsed");
}

#[test]
fn test_sql_signature_params_ignore_header_comments() {
    let source = r#"
CREATE PROCEDURE [dbo].[usp_HeaderComments]
    -- AS SELECT @LegacyParam
    /* @BlockParam BIGINT */
    @RealParam INT
AS
BEGIN
    SELECT @BodyLocal = 1
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let proc = defs.iter().find(|d| d.kind == DefinitionKind::StoredProcedure).unwrap();
    let sig = proc.signature.as_ref().unwrap();

    assert!(sig.contains("(@RealParam)"), "Signature should include real header param, got: {}", sig);
    assert!(!sig.contains("@LegacyParam"), "Signature should ignore line-comment params, got: {}", sig);
    assert!(!sig.contains("@BlockParam"), "Signature should ignore block-comment params, got: {}", sig);
    assert!(!sig.contains("@BodyLocal"), "Signature should not include body values, got: {}", sig);
}


#[test]
fn test_sql_signature_params_preserve_crlf_boundaries() {
    let source = "CREATE PROCEDURE [dbo].[usp_Crlf]\r\n    @FirstParam INT\r\n    ,@SecondParam BIGINT\r\nAS\r\nBEGIN\r\n    SELECT @BodyLocal = 1\r\nEND\r\n";
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let proc = defs.iter().find(|d| d.kind == DefinitionKind::StoredProcedure).unwrap();
    let sig = proc.signature.as_ref().unwrap();

    assert!(sig.contains("(@FirstParam, @SecondParam)"), "Signature should include CRLF header params, got: {}", sig);
    assert!(!sig.contains("@BodyLocal"), "Signature should not include CRLF body values, got: {}", sig);
}

#[test]
fn test_sql_whitespace_only_file() {
    let source = "   \n\n   \n  \t  \n";
    let (defs, _, _) = parse_sql_definitions(source, 0);
    assert!(defs.is_empty(), "Whitespace-only file should produce 0 definitions");
}

#[test]
fn test_sql_code_stats_populated() {
    let source = r#"
CREATE PROCEDURE [dbo].[usp_Simple]
AS
BEGIN
    SELECT 1
END
"#;
    let (defs, _, code_stats) = parse_sql_definitions(source, 0);

    assert_eq!(defs.len(), 1);
    assert_eq!(code_stats.len(), 1, "Should have code stats for the procedure");
    let (idx, stats) = &code_stats[0];
    assert_eq!(*idx, 0);
    assert!(stats.cyclomatic_complexity > 0, "Code stats should have non-zero complexity");
}

#[test]
fn test_sql_index_without_schema() {
    let source = r#"
CREATE INDEX IX_Simple ON Orders (CustomerId ASC)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let idx_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::SqlIndex).collect();
    assert_eq!(idx_defs.len(), 1);
    assert_eq!(idx_defs[0].name, "IX_Simple");
    assert_eq!(idx_defs[0].parent, Some("Orders".to_string()));
}

#[test]
fn test_sql_create_or_alter_function() {
    let source = r#"
CREATE OR ALTER FUNCTION [dbo].[udf_GetFullName]
(
    @FirstName NVARCHAR(100),
    @LastName NVARCHAR(100)
)
RETURNS NVARCHAR(201)
AS
BEGIN
    RETURN @FirstName + ' ' + @LastName
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let func_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::SqlFunction).collect();
    assert_eq!(func_defs.len(), 1, "Expected 1 function for CREATE OR ALTER FUNCTION");
    assert_eq!(func_defs[0].name, "udf_GetFullName");
}

#[test]
fn test_sql_create_or_alter_view() {
    let source = r#"
CREATE OR ALTER VIEW [dbo].[vw_ActiveOrders]
AS
SELECT * FROM [dbo].[Orders] WHERE [StatusCode] = 1
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let view_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::View).collect();
    assert_eq!(view_defs.len(), 1, "Expected 1 view for CREATE OR ALTER VIEW");
    assert_eq!(view_defs[0].name, "vw_ActiveOrders");
}

#[test]
fn test_sql_multiple_fk_same_table() {
    // Multiple FK references to the same table should be deduplicated in base_types
    let source = r#"
CREATE TABLE [dbo].[OrderHistory]
(
    [HistoryId] BIGINT NOT NULL,
    [OriginalOrderId] BIGINT NOT NULL,
    [ReplacementOrderId] BIGINT NULL,
    CONSTRAINT [FK_History_OrigOrder] FOREIGN KEY ([OriginalOrderId]) REFERENCES [dbo].[Orders] ([OrderId]),
    CONSTRAINT [FK_History_ReplOrder] FOREIGN KEY ([ReplacementOrderId]) REFERENCES [dbo].[Orders] ([OrderId])
)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);

    let table = defs.iter().find(|d| d.kind == DefinitionKind::Table).unwrap();
    // "Orders" should appear only once in base_types
    let orders_count = table.base_types.iter().filter(|bt| *bt == "Orders").count();
    assert_eq!(orders_count, 1,
        "Duplicate FK references to same table should be deduplicated in base_types, got {} occurrences",
        orders_count);
}
// ─── Test: Comment header before CREATE PROCEDURE ──────────────────
// Regression test: files with comment banners (dashes, copyright) before
// the CREATE statement were not parsed because the regex used `^` anchor
// which only matched at the start of the batch text (no multiline flag).

#[test]
fn test_sql_comment_header_before_create() {
    let source = r#"--------------------------------------------------------------
-- Copyright (c) Contoso Ltd.
--------------------------------------------------------------
CREATE PROCEDURE [Modifiers].[usp_GetUserMapping_V5]
    @UserObjectId      AS NVARCHAR (256),
    @IndexType           AS INT,
    @IndexName           AS NVARCHAR (256)
AS
BEGIN
    SET NOCOUNT ON
    SELECT * FROM [dbo].[UserMappings]
    WHERE [UserObjectId] = @UserObjectId
END
"#;
    let (defs, call_sites, _) = parse_sql_definitions(source, 0);

    let proc_defs: Vec<_> = defs.iter().filter(|d| d.kind == DefinitionKind::StoredProcedure).collect();
    assert_eq!(proc_defs.len(), 1, "Expected 1 stored procedure from file with comment header");
    assert_eq!(proc_defs[0].name, "usp_GetUserMapping_V5");

    // Should also extract call sites
    assert!(!call_sites.is_empty(), "Expected call sites from SP body");
    let (_, calls) = &call_sites[0];
    let call_names: Vec<&str> = calls.iter().map(|c| c.method_name.as_str()).collect();
    assert!(call_names.contains(&"UserMappings"),
        "Expected FROM call to UserMappings, got: {:?}", call_names);
}

// ─── Defensive coding tests: corrupted SQL must not panic ──────────

/// Truncated CREATE TABLE — name parsed but body is incomplete.
/// Parser should NOT panic on missing capture groups.
#[test]
fn test_sql_corrupted_truncated_create_table() {
    // Truncated right after TABLE keyword — no name follows
    let source = "CREATE TABLE";
    let (defs, calls, stats) = parse_sql_definitions(source, 0);
    // Should not panic — 0 definitions is acceptable
    assert!(defs.is_empty() || !defs.is_empty(), "Should not panic on truncated CREATE TABLE");
    let _ = (calls, stats); // suppress unused warnings
}

/// CREATE TABLE with schema dot but missing table name.
#[test]
fn test_sql_corrupted_schema_dot_no_name() {
    let source = "CREATE TABLE [dbo].";
    let (defs, _, _) = parse_sql_definitions(source, 0);
    // May produce 0 defs or a def with empty name — either is fine, just no panic
    for d in &defs {
        assert!(!d.name.is_empty() || defs.is_empty(),
            "Should not produce a definition with empty name from corrupted SQL");
    }
}

/// FK REFERENCES with schema dot but no table name.
#[test]
fn test_sql_corrupted_fk_reference_incomplete() {
    let source = r#"
CREATE TABLE [dbo].[Items]
(
    [Id] INT NOT NULL,
    CONSTRAINT [FK_Bad] FOREIGN KEY ([Id]) REFERENCES [dbo].
)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);
    // Should parse the table, FK extraction may fail gracefully
    let table = defs.iter().find(|d| d.kind == DefinitionKind::Table);
    assert!(table.is_some(), "Table should still be parsed despite corrupted FK");
}

/// CREATE INDEX with ON clause but missing table name.
#[test]
fn test_sql_corrupted_index_no_table() {
    let source = "CREATE INDEX IX_Bad ON";
    let (defs, _, _) = parse_sql_definitions(source, 0);
    // Should not panic
    let _ = defs;
}

/// Procedure with no parameters and garbled body — call site extraction must not panic.
#[test]
fn test_sql_corrupted_procedure_garbled_body() {
    let source = r#"
CREATE PROCEDURE [dbo].[usp_Broken]
AS
BEGIN
    SELECT FROM [dbo]. WHERE = AND
    EXEC [].[]
    INSERT INTO
    UPDATE SET
    DELETE FROM
END
"#;
    let (defs, call_sites, code_stats) = parse_sql_definitions(source, 0);
    assert_eq!(defs.len(), 1, "Should parse the procedure definition");
    assert_eq!(defs[0].name, "usp_Broken");
    // Call sites may be empty or partial — just no panic
    let _ = (call_sites, code_stats);
}

/// Binary/garbled content that looks vaguely like SQL but isn't.
#[test]
fn test_sql_corrupted_binary_content() {
    let source = "CREATE\x00TABLE\x00[bad]\x00\x01\x02\x03";
    let (defs, calls, stats) = parse_sql_definitions(source, 0);
    let _ = (defs, calls, stats); // just verify no panic
}

/// Multiple consecutive GO delimiters with empty batches.
#[test]
fn test_sql_corrupted_empty_go_batches() {
    let source = "GO\nGO\nGO\nGO\n";
    let (defs, calls, stats) = parse_sql_definitions(source, 0);
    assert!(defs.is_empty());
    let _ = (calls, stats);
}

/// CREATE PROCEDURE with unmatched brackets in name.
#[test]
fn test_sql_corrupted_unmatched_brackets() {
    let source = r#"
CREATE PROCEDURE [dbo.[usp_BadBrackets
AS
BEGIN
    SELECT 1
END
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);
    // May or may not parse — just no panic
    let _ = defs;
}

/// Column regex edge case: line looks like a column but has weird types.
#[test]
fn test_sql_corrupted_column_weird_types() {
    let source = r#"
CREATE TABLE [dbo].[WeirdTable]
(
    [Normal] INT NOT NULL,
    [Broken NOTACOLUMN,
    [] INT,
    [  ] NVARCHAR(10),
    CONSTRAINT
)
"#;
    let (defs, _, _) = parse_sql_definitions(source, 0);
    // Should parse the table, columns may be partial
    let table = defs.iter().find(|d| d.kind == DefinitionKind::Table);
    assert!(table.is_some(), "Table should be parsed despite corrupted columns");
}
