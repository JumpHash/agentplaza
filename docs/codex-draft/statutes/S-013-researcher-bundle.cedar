// S-013: Researcher bundle (0028, 0117). Broad read access to the web; everything it produces is untrusted.

@law("S-013") @clause("a") @tier("statute") @enforcement("preventive")
@title("Researchers may read the open web")
@statement("A researcher may connect to any host except blocked ones. Its output is always tainted.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"egress.connect", resource is Host)
when { principal.role == "researcher" && !(resource in HostGroup::"blocked") };

@law("S-013") @clause("b") @tier("statute") @enforcement("preventive")
@title("Researchers may use web search")
@statement("A researcher may call web-search tools (e.g. Firecrawl).")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"tool.call", resource)
when { principal.role == "researcher" && resource in ToolGroup::"web-search" };

@law("S-013") @clause("c") @tier("statute") @enforcement("preventive")
@title("Researchers submit research briefs")
@statement("A researcher may submit research briefs to its own town.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"research.submit", resource)
when { principal.role == "researcher" && resource == principal.town };

@law("S-013") @clause("d") @tier("statute") @enforcement("preventive")
@title("Researchers never write to repositories")
@statement("A researcher may not connect to the hub git: untrusted content must reach code only through a builder who treats it as data.")
@severity("high") @sanctions("scope-suspension; probation:7d") @appealable("yes")
forbid (principal is Agent, action == Action::"egress.connect", resource)
when { principal.role == "researcher" && resource in HostGroup::"hub-git" };
