# One entry per started scenario: its name and the tags it carries.
select(.type == "scenario_started") | "\(.name)=\(.tags | join(","))"
