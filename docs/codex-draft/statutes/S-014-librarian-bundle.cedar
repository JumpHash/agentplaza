// S-014: Librarian bundle (0068, 0117).

@law("S-014") @clause("a") @tier("statute") @enforcement("preventive")
@title("Librarians read memory at every scope")
@statement("A librarian may read private, group and org memory to curate it.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"memory.read", resource)
when { principal.role == "librarian" };

@law("S-014") @clause("b") @tier("statute") @enforcement("preventive")
@title("Librarians change memory only on verified evidence")
@statement("A librarian may promote, merge or expire memories only when the originating tasks passed verification; tainted memories never reach org scope without owner review.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"memory.change", resource)
when {
  principal.role == "librarian" && context.verified &&
  (context.to_scope != "org" || !context.tainted || context.owner_reviewed)
};
