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

def read_data_from_batch(folder_name, file_count):
    total_integer_list = []  # List to store integer values from each file

    for i in range(1, file_count + 1):
        filename = f'{folder_name}/output_{i}.txt'
        integer_list = []  # List to store integer values from the current file

        with open(filename, 'r') as file:
            lines = file.readlines()

            # Skip the first four lines
            lines = lines[6:]

            # Convert the remaining lines to integers and append them to the list
            for line in lines:
                integer_list.append(int(line.strip()))
        total_integer_list += integer_list
    total_integer_list = [x / 1000 for x in total_integer_list]
    print(f"Done reading: {folder_name}")
    return total_integer_list

plt.style.use('tableau-colorblind10')
plt.rcParams["font.family"] = 'serif'

# Read data from different batches
data_batch_16 = read_data_from_batch('batch_16', 1000)
data_batch_32 = read_data_from_batch('batch_32', 1000)
data_batch_64 = read_data_from_batch('batch_64', 1000)

lower_x_limit = min(data_batch_64 + data_batch_16 + data_batch_32) - 0.1
lower_x_limit = 27.5
upper_x_limit = 41.5

# Create a figure and plot the histograms
fig, ax = plt.subplots()

# Plot the data from batch_16
n_batch_16 = np.arange(1, len(data_batch_16) + 1) / np.float64(len(data_batch_16))
data_s_batch_16 = np.sort(data_batch_16)
ax.step(data_s_batch_16, n_batch_16, label='Batch size 16')
print("Done plotting 16")

# Plot the data from batch_32
n_batch_32 = np.arange(1, len(data_batch_32) + 1) / np.float64(len(data_batch_32))
data_s_batch_32 = np.sort(data_batch_32)
ax.step(data_s_batch_32, n_batch_32, label='Batch size 32')
print("Done plotting 32")

# Plot the data from batch_64
n_batch_64 = np.arange(1, len(data_batch_64) + 1) / np.float64(len(data_batch_64))
data_s_batch_64 = np.sort(data_batch_64)
ax.step(data_s_batch_64, n_batch_64, label='Batch size 64')
print("Done plotting 64")

# Add a legend to the plot
ax.legend()

# Add labels and title
ax.set_xlabel('Latency (μs)', fontsize=axis_label_fontsize)
ax.set_title('Cumulative Epoch Latency Distributions', fontsize=title_fontsize)
ax.tick_params(axis='both', labelsize=tick_label_fontsize)
ax.grid(axis='y', linestyle='-.')
ax.set_xlim(lower_x_limit, upper_x_limit)

ratio = 0.5
x_left, x_right = ax.get_xlim()
y_low, y_high = ax.get_ylim()
ax.set_aspect(abs((x_right - x_left) / (y_low - y_high)) * ratio)

fig.show()
fig.savefig("../epoch_latencies_cdf.pdf", bbox_inches='tight', pad_inches = 0)
