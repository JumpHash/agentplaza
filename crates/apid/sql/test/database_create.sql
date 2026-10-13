-- Test only: creates a throwaway database. The name cannot be a parameter (it is an identifier),
-- so the caller substitutes {name} after checking it is a lowercase identifier (a-z, 0-9, _).
CREATE DATABASE {name}
