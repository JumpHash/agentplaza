; CSS: rules by their selectors, custom properties (--token), and where a custom property is read.

(rule_set (selectors) @name) @definition.rule
((declaration (property_name) @name) @definition.variable (#match? @name "^--"))
((call_expression (function_name) @_f (arguments (plain_value) @name)) @reference.variable (#eq? @_f "var"))
