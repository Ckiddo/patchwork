"""Explicit boundary states; not represented as histories of legal full games.

Every installed state must pass game_core's deserializer and invariant checks.
"""
import copy
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CATALOG = {p['id']:p for p in json.loads((ROOT/'game_core/tests/fixtures/patchwork_custom_v1.json').read_text())['patches']}
BOARDS = json.loads((ROOT/'game_core/tests/fixtures/t37_boards.json').read_text())


def cells_for(id, turns=0, flipped=False):
    cells = [(p['x'],p['y']) if isinstance(p,dict) else tuple(p) for p in CATALOG[id]['cells']]
    if flipped: cells = [(-x,y) for x,y in cells]
    for _ in range(turns): cells = [(-y,x) for x,y in cells]
    mx,my = min(x for x,y in cells),min(y for x,y in cells)
    return sorted(((x-mx,y-my) for x,y in cells),key=lambda c:(c[1],c[0]))


def empty(source):
    s = copy.deepcopy(source)
    s.update(lifecycle='running', result=None, bonus={'owner':None}, action={'last_normal_actor':1,'pending_specials':[]})
    for p in s['players']:
        p.update(buttons=20,income=0,time_position=0,placed_pieces=[],board={'cells':[[None]*9 for _ in range(9)]})
    order=s['supply']['initial_order']
    s['supply'].update(remaining=order.copy(),neutral={'kind':'before_slot','slot':order.index(10)})
    for item in s['special_patches']: item['status']={'kind':'available'}
    return s


def add(s, seat, id, x, y, turns=0, flipped=False, special=False):
    piece={'kind':'special' if special else 'normal','id':id}
    offsets=[(0,0)] if special else cells_for(id,turns,flipped)
    cells=[{'x':x+dx,'y':y+dy} for dx,dy in offsets]
    p=s['players'][seat]
    for c in cells:
        assert 0<=c['x']<9 and 0<=c['y']<9 and p['board']['cells'][c['y']][c['x']] is None
        p['board']['cells'][c['y']][c['x']]=piece.copy()
    p['placed_pieces'].append({'piece':piece,'cells':cells,'quarter_turns':turns,'flipped':flipped})
    if not special:
        p['income']+=CATALOG[id]['income']
        slot=s['supply']['initial_order'].index(id)
        assert s['supply']['remaining'][slot]==id
        s['supply']['remaining'][slot]=None
        s['supply']['neutral']={'kind':'on_vacated_slot','slot':slot}


def tiled(s, seat, key):
    for p in BOARDS[key]: add(s,seat,**p)


def pending(s,seat):
    s['action']['last_normal_actor']=seat
    for item in s['special_patches']:
        if item['track_position']<=s['players'][seat]['time_position'] and item['status']['kind']=='available':
            item['status']={'kind':'pending','owner':seat}
            s['action']['pending_specials'].append({'owner':seat,'track_position':item['track_position']})


def completed(board):
    return any(all(board[y+dy][x+dx] is not None for dy in range(7) for dx in range(7)) for y in range(3) for x in range(3))


def candidates_at(s,id):
    slot=s['supply']['initial_order'].index(id)
    for distance in range(1,34):
        prev=(slot-distance)%33
        if s['supply']['remaining'][prev] is None:
            s['supply']['neutral']={'kind':'on_vacated_slot','slot':prev}
            return
    raise AssertionError('fixture needs a vacated slot')


def make(source,kind):
    s=empty(source)
    if kind=='bonus_normal':
        tiled(s,0,'square_missing_domino')
        s['players'][1]['time_position']=1
        candidates_at(s,10)
    elif kind=='bonus_challenger':
        s=copy.deepcopy(source)
        assert s['bonus']['owner']==0 and not s['players'][1]['placed_pieces']
        tiled(s,1,'other_square_missing_special')
        s['players'][0]['time_position']=19
        s['players'][1]['time_position']=19
        pending(s,1)
    elif kind=='bonus_special':
        tiled(s,1,'other_square_missing_special')
        s['players'][0]['time_position']=18
        s['players'][1]['time_position']=19
        pending(s,1)
    elif kind=='full_pending':
        tiled(s,0,'board_missing_center')
        for p in s['players']: p.update(time_position=53,buttons=5)
        pending(s,0)
    elif kind=='end_draw':
        add(s,0,12,0,0)
        s['players'][0].update(time_position=51,buttons=0)
        s['players'][1].update(time_position=53,buttons=4)
        for i,item in enumerate(s['special_patches']):
            add(s,1,item['track_position'],i,8,special=True)
            item['status']={'kind':'placed','owner':1,'position':{'x':i,'y':8}}
    elif kind in ('supply_two','supply_one'):
        packing=json.loads((ROOT/'game_core/tests/fixtures/t40_packing.json').read_text())
        # Put the last two patches across the ring boundary with empty slots between them.
        order=[p for p in s['supply']['initial_order'] if p not in (10,27)]
        order.insert(1,27)
        order.append(10)
        s['supply'].update(initial_order=order,remaining=order.copy())
        for p in packing['pieces']:
            if kind=='supply_two' and p['id']==10: continue
            add(s,p['seat'],p['id'],p['x'],p['y'],p['turns'],p['flipped'])
        for p in s['players']: p['buttons']=100
        s['players'][1]['time_position']=1
        for i,p in enumerate(s['players']):
            if completed(p['board']['cells']): s['bonus']['owner']=i; break
        s['supply']['neutral']={'kind':'on_vacated_slot','slot':30}
    else: raise ValueError('unknown fixture')
    return s
