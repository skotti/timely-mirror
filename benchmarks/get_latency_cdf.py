#!/usr/bin/env python3
#%%
import sys
import pandas as pd
import numpy as np
import matplotlib.pyplot as plt
from matplotlib.patches import Polygon
# from util import adjustFigAspect
from matplotlib import rcParams
from cycler import cycler

title_fontsize = 16
axis_label_fontsize = 14
tick_label_fontsize = 12
legend_fontsize = 12
legend_alpha = 0.9
bar_alpha = 1.0
bar_aspect = 0.7
def draw_rpc_cdf():
    plt.style.use('tableau-colorblind10')
    plt.rcParams["font.family"] = 'serif'
    # data = [2490, 2420, 2390, 2420, 2440, 2410, 2430, 2470, 2410, 2420,
    #          2410, 2440, 2410, 2400, 2400, 2420, 2450, 2430, 2430, 2410,
    #          2430, 2400, 2390, 2410, 2440, 2430, 2420, 2430, 2420, 2430,
    #          2440, 2430, 2400, 2430, 2450, 2410, 2430, 2430, 2420, 2430,
    #          2440, 2430, 2440, 2400, 2420, 2420, 2410, 2430, 2430, 2400,
    #          2420, 2450, 2410, 2440, 2410, 2440, 2420, 2430, 2410, 2390,
    #          2400, 2430, 2430, 2400, 2410, 2440, 2400, 2420, 2430, 2420,
    #          2440, 2410, 2410, 2430, 2430, 2430, 2430, 2400, 2440, 2400,
    #          2400, 2390, 2440, 2440, 2410, 2420, 2410, 2440, 2430, 2430,
    #          2400, 2440, 2440, 2430, 2430, 2400, 2440, 2410, 2430, 2410]

    
    file_count = 1000  # Number of files to read
    total_integer_list = []  # List to store integer values from each file

    for i in range(1, file_count + 1):
        filename = f'data-v2/output_{i}.txt'
        integer_list = []  # List to store integer values from the current file

        with open(filename, 'r') as file:
            lines = file.readlines()

            # Skip the first four lines
            lines = lines[4:]

            # Convert the remaining lines to integers and append them to the list
            for line in lines:
                integer_list.append(int(line.strip()))
        total_integer_list += integer_list
    total_integer_list = [x / 1000 for x in total_integer_list]
    data = total_integer_list

    upper_x_limit = 30

    print(len([x for x in total_integer_list if x > upper_x_limit]) / len(total_integer_list))

    print(len(data))

    # df = pd.read_csv("./output_data.csv")["epoch_latency_0_5"]
    # data = df
    # data = data / 2

    # Create a figure and plot the histograms
    fig, ax = plt.subplots()
    #ax.hist(data1, bins=100, alpha=1, label='Writes')
    #ax.hist(data2, bins=100, alpha=1, label='Reads')
    n = np.arange(1,len(data)+1) / np.float64(len(data))
    data_s = np.sort(data)
    #fig, ax = plt.subplots()
    ax.step(data_s,n) 
    #x.hist(data_s, histtype='step', cumulative=True, alpha=1, label='CDF')

    # Add a legend to the plot
    #ax.legend()
    
    # Add labels and title
    ax.set_xlabel('Latency (μs)', fontsize=axis_label_fontsize)
    #ax.set_title('Distribution of the latency of RPC calls', fontsize=title_fontsize)
    ax.tick_params(axis='both', labelsize=tick_label_fontsize)
    #ax.set_ylabel('Throughput (GiB/s)', fontsize=axis_label_fontsize)
    ax.set_title('Epoch Latency Distribution',fontsize=title_fontsize)
    ax.grid(axis='y', linestyle='-.')
    ax.set_xlim(min(data)-0.1, upper_x_limit)
    # ax.set_xlim(1100,1300)
    ratio = 0.5
    x_left, x_right = ax.get_xlim()
    y_low, y_high = ax.get_ylim()
    ax.set_aspect(abs((x_right-x_left)/(y_low-y_high))*ratio)
    
    fig.show()
    
    fig.savefig("../epoch_latency_cdf.pdf", bbox_inches='tight', pad_inches = 0)
#fig.savefig('preliminary_distribution_2.png')
# Show the plot

#    plt.hist(put_data_here, normed=True, cumulative=True, label='CDF',
#        histtype='step', alpha=0.8, color='k')
draw_rpc_cdf()

