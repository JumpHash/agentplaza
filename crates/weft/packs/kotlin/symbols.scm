; Kotlin: classes, interfaces, objects, functions and methods, and the functions a file calls.

(class_declaration name: (identifier) @name) @definition.class
(object_declaration name: (identifier) @name) @definition.class
(function_declaration name: (identifier) @name) @definition.function

(call_expression (identifier) @name) @reference.call
(call_expression (navigation_expression (identifier) @name .)) @reference.call
