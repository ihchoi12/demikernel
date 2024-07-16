import os
import pandas as pd
import numpy as np

################## PATHS #####################
HOME = os.path.expanduser("~")
LOCAL = HOME.replace("/homes", "/local")
AUTOKERNEL_PATH = f'{HOME}/Capybara/demikernel'
CALADAN_PATH = f'{HOME}/Capybara/caladan'
DATA_PATH = f'{HOME}/autokernel-data'
PCAP_PATH = f'{LOCAL}/autokernel-pcap'

################## CLUSTER CONFIG #####################
ALL_NODES = ['node1', 'node7', 'node8', 'node9']
CLIENT_NODE = 'node7'
FRONTEND_NODE = 'node8'
BACKEND_NODE = 'node8'
TCPDUMP_NODE = 'node1'
NODE8_IP = '10.0.1.8'
NODE8_MAC = '08:c0:eb:b6:e8:05'
NODE9_IP = '10.0.1.9'
NODE9_MAC = '08:c0:eb:b6:c5:ad'

################## BUILD CONFIG #####################
LIBOS = 'catnip'#'catnap', 'catnip'
FEATURES = [
    # 'tcp-migration',
    # 'manual-tcp-migration',
    # 'capy-log',
    # 'capy-profile',
    # 'capy-time-log',
    # 'server-reply-analysis',
]

################## TEST CONFIG #####################
NUM_BACKENDS = 1
SERVER_APP = 'http-server' # 'http-server', 'prism', 'redis-server', 'proxy-server'
CLIENT_APP = 'caladan' # 'wrk', 'caladan'
NUM_THREADS = [1] # for wrk load generator
REPEAT_NUM = 1

EVAL_LATENCY_TRACE = False
EVAL_SERVER_REPLY = False
TCPDUMP = False

################## ENV VARS #####################
### SERVER ###
SESSION_DATA_SIZE = 1024 * 0 # bytes

CAPY_LOG = 'all' # 'all', 'mig'
REDIS_LOG = 0


### CALADAN ###
CLIENT_PPS = [i for i in range(10000, 400000 + 1, 30000)]#[i for i in range(100000, 1_300_001, 100000)]
LOADSHIFTS = ''
# LOADSHIFTS = '90000:10000,270000:10000,450000:10000,630000:10000,810000:10000/90000:50000/90000:50000/90000:50000'
# LOADSHIFTS = ''#'10000:10000/10000:10000/10000:10000/10000:10000'
ZIPF_ALPHA = '' # 0.9
ONOFF = '0' # '0', '1'
NUM_CONNECTIONS = [1]
RUNTIME = 10

#####################
# build command: run_eval.py [build [clean]]
# builds based on SERVER_APP
# cleans redis before building if clean