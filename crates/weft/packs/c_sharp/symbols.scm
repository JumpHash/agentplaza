; C#: types, members and namespaces, and the names they call or create.

(class_declaration name: (identifier) @name) @definition.class
(struct_declaration name: (identifier) @name) @definition.struct
(record_declaration name: (identifier) @name) @definition.class
(interface_declaration name: (identifier) @name) @definition.interface
(enum_declaration name: (identifier) @name) @definition.enum
(delegate_declaration name: (identifier) @name) @definition.type
(method_declaration name: (identifier) @name) @definition.method
(constructor_declaration name: (identifier) @name) @definition.method
(property_declaration name: (identifier) @name) @definition.property
(namespace_declaration name: (_) @name) @definition.module
(file_scoped_namespace_declaration name: (_) @name) @definition.module

(invocation_expression function: (identifier) @name) @reference.call
(invocation_expression function: (member_access_expression name: (identifier) @name)) @reference.call
(object_creation_expression type: (identifier) @name) @reference.type
(base_list (identifier) @name) @reference.type
