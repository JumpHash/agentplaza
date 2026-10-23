; PowerShell: classes, their methods, and functions, and the commands and methods a file calls.

(class_statement (simple_name) @name) @definition.class
(class_method_definition (simple_name) @name) @definition.method
(function_statement (function_name) @name) @definition.function

(command command_name: (command_name) @name) @reference.call
(invokation_expression (member_access (member_name (simple_name) @name))) @reference.call
