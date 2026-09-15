"""Generate a dense two-board fixture with an optional workspace-only CP-SAT dependency."""
import json
from pathlib import Path
import sys

ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT/'artifacts/t40-python'))
from ortools.sat.python import cp_model
from t40_fixtures import BOARDS,CATALOG,cells_for

model=cp_model.CpModel()
prefix=[dict(p,seat=0) for p in BOARDS['board_missing_center']]
fixed={p['id'] for p in prefix}
excluded=max((id for id in CATALOG if id not in fixed),key=lambda id:len(CATALOG[id]['cells']))
variables=[]
by_cell={(s,x,y):[] for s in range(2) for y in range(9) for x in range(9)}
for id in CATALOG:
    if id==excluded or id in fixed: continue
    choices=[]
    seen=set()
    for flip in (False,True):
        for turns in range(4):
            offsets=tuple(cells_for(id,turns,flip))
            if offsets in seen: continue
            seen.add(offsets)
            width=max(x for x,y in offsets)+1; height=max(y for x,y in offsets)+1
            for seat in [1]:
                for y in range(10-height):
                    for x in range(10-width):
                        var=model.new_bool_var('')
                        choices.append(var)
                        for dx,dy in offsets: by_cell[seat,x+dx,y+dy].append(var)
                        variables.append((var,{'seat':seat,'id':id,'x':x,'y':y,'turns':turns,'flipped':flip}))
    model.add_exactly_one(choices)
for choices in by_cell.values(): model.add_at_most_one(choices)
solver=cp_model.CpSolver()
solver.parameters.max_time_in_seconds=120
solver.parameters.num_search_workers=4
status=solver.solve(model)
assert status in (cp_model.OPTIMAL,cp_model.FEASIBLE), solver.status_name(status)
pieces=prefix+[p for v,p in variables if solver.boolean_value(v)]
result={'excluded':excluded,'area':sum(len(CATALOG[p['id']]['cells']) for p in pieces),'pieces':pieces,'solver':solver.status_name(status),'seconds':solver.wall_time}
(ROOT/'artifacts/t40-packing.json').write_text(json.dumps(result,indent=2))
print(json.dumps({k:v for k,v in result.items() if k!='pieces'}))
