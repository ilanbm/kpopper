"""Read the explicit visibility switch without importing application code."""
import ast
import json
from pathlib import Path

assignments = [node for node in ast.parse(Path("search.py").read_text()).body
               if isinstance(node, ast.Assign)
               and any(isinstance(t, ast.Name) and t.id == "INCLUDE_PRIVATE_PROJECTS"
                       for t in node.targets)]
if len(assignments) != 1:
    raise ValueError("Expected one explicit INCLUDE_PRIVATE_PROJECTS switch")
includes_private = ast.literal_eval(assignments[0].value)
if type(includes_private) is not bool:
    raise ValueError("The visibility switch must be a boolean")
print(json.dumps(not includes_private))
