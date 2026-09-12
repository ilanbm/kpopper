INCLUDE_PRIVATE_PROJECTS = True
PROJECTS = [
    {"name": "public project", "private": False, "owner": None},
    {"name": "private project", "private": True, "owner": "alice"},
]


def search(query, user):
    return [dict(project) for project in PROJECTS
            if query in project["name"]
            and (not project["private"]
                 or (INCLUDE_PRIVATE_PROJECTS and project["owner"] == user))]
