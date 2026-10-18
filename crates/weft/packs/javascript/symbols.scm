; JavaScript.

(function_declaration name: (identifier) @name) @definition.function
(generator_function_declaration name: (identifier) @name) @definition.function
(class_declaration name: (identifier) @name) @definition.class
(method_definition name: (property_identifier) @name) @definition.method
(variable_declarator name: (identifier) @name value: [(arrow_function) (function_expression)]) @definition.function
(program (lexical_declaration (variable_declarator name: (identifier) @name value: [(new_expression) (call_expression) (object) (array) (string) (number) (template_string) (member_expression) (binary_expression) (true) (false)])) @definition.constant)
(program (export_statement (lexical_declaration (variable_declarator name: (identifier) @name value: [(new_expression) (call_expression) (object) (array) (string) (number) (template_string) (member_expression) (binary_expression) (true) (false)])) @definition.constant))

(call_expression function: (identifier) @name) @reference.call
(call_expression function: (member_expression property: (property_identifier) @name)) @reference.call
(new_expression constructor: (identifier) @name) @reference.class
