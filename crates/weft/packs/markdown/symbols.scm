; Markdown: sections by their headings, and list items (a document's claims).
(section (atx_heading heading_content: (inline) @name)) @definition.section
(section (setext_heading (paragraph (inline) @name))) @definition.section
(list_item (paragraph (inline) @name)) @definition.item
